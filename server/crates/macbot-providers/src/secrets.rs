//! Opt-in development credentials outside the checkout. Production defaults to
//! Keychain; neither backend puts credential values in provider snapshots.

use crate::{Error, Result, SecretStore};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{fs, io::Write, path::PathBuf, sync::Arc};

pub struct FileSecrets {
    directory: PathBuf,
}

fn storage_error(_: std::io::Error) -> Error {
    // Do not include file contents or OS diagnostics that may contain secrets.
    Error::Secret("development credential file operation failed".into())
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
                "development credential directory must be a real directory".into(),
            ));
        }
        let directory = fs::canonicalize(&absolute).map_err(storage_error)?;
        if directory
            .ancestors()
            .any(|parent| parent.join(".git").exists())
        {
            return Err(Error::Secret(
                "development credentials must be outside a Git checkout".into(),
            ));
        }
        #[cfg(unix)]
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(storage_error)?;
        Ok(Self { directory })
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.len() > 256 {
            return Err(Error::Secret("invalid credential identifier".into()));
        }
        // Hex encoding is reversible and collision-free, including namespaced
        // identifiers; a provider id can never become a path component.
        let name = id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(self.directory.join(format!("{name}.key")))
    }
}

impl SecretStore for FileSecrets {
    fn get(&self, id: &str) -> Result<Option<String>> {
        let path = self.path(id)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                #[cfg(unix)]
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                    .map_err(storage_error)?;
                fs::read_to_string(path).map(Some).map_err(storage_error)
            }
            Ok(_) => Err(Error::Secret(
                "credential file must be a regular file".into(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage_error(error)),
        }
    }

    fn set(&self, id: &str, key: &str) -> Result<()> {
        let destination = self.path(id)?;
        let temporary = self
            .directory
            .join(format!(".{}.tmp", uuid::Uuid::now_v7()));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary).map_err(storage_error)?;
            file.write_all(key.as_bytes()).map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            fs::rename(&temporary, &destination).map_err(storage_error)?;
            fs::File::open(&self.directory)
                .and_then(|directory| directory.sync_all())
                .map_err(storage_error)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn delete(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.path(id)?) {
            Ok(()) => fs::File::open(&self.directory)
                .and_then(|directory| directory.sync_all())
                .map_err(storage_error),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage_error(error)),
        }
    }
}

pub fn configured_secret_store() -> Result<Arc<dyn SecretStore>> {
    match std::env::var("MACBOT_SECRET_BACKEND").as_deref() {
        Ok("file") => {
            let directory = match std::env::var_os("MACBOT_SECRET_DIR") {
                Some(directory) => PathBuf::from(directory),
                None => PathBuf::from(std::env::var_os("HOME").ok_or_else(|| {
                    Error::Secret("HOME is required for development credential storage".into())
                })?)
                .join("MacBot-dev-secrets"),
            };
            Ok(Arc::new(FileSecrets::new(directory)?))
        }
        Ok("keychain") | Err(std::env::VarError::NotPresent) => {
            #[cfg(target_os = "macos")]
            {
                Ok(Arc::new(crate::KeychainSecrets))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Ok(Arc::new(crate::MemorySecrets::default()))
            }
        }
        _ => Err(Error::Secret("unsupported MACBOT_SECRET_BACKEND".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_files_survive_reopen_and_replace_without_leaking_paths() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileSecrets::new(directory.path().join("credentials")).unwrap();
        let id = "../provider/namespaced";
        assert_eq!(store.get(id).unwrap(), None);
        store.set(id, "fake-token").unwrap();
        store.set(id, "replacement-fake-token").unwrap();
        let reopened = FileSecrets::new(directory.path().join("credentials")).unwrap();
        assert_eq!(
            reopened.get(id).unwrap().as_deref(),
            Some("replacement-fake-token")
        );
        let files = fs::read_dir(directory.path().join("credentials"))
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1);
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
            assert_eq!(
                fs::metadata(reopened.path(id).unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        reopened.delete(id).unwrap();
        reopened.delete(id).unwrap();
        assert_eq!(store.get(id).unwrap(), None);
    }

    #[test]
    fn development_store_rejects_checkouts_and_symlink_reads() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".git"), "gitdir: fake").unwrap();
        assert!(FileSecrets::new(directory.path().join("keys")).is_err());
        fs::remove_file(directory.path().join(".git")).unwrap();
        let store = FileSecrets::new(directory.path().join("keys")).unwrap();
        #[cfg(unix)]
        {
            let target = directory.path().join("outside");
            fs::write(&target, "fake-token").unwrap();
            std::os::unix::fs::symlink(&target, store.path("p").unwrap()).unwrap();
            assert!(store.get("p").is_err());
            store.set("p", "replacement").unwrap();
            assert_eq!(fs::read_to_string(target).unwrap(), "fake-token");
        }
    }
}
