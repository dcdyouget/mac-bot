//! Shared paths for client-local data.
//!
//! `MACBOT_CLIENT_DATA_DIR` is intended for isolated development and QA
//! instances. When it is unset, paths retain the normal macOS location.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

const DATA_DIR_ENV: &str = "MACBOT_CLIENT_DATA_DIR";
const DEFAULT_RELATIVE_DIR: &str = "Library/Application Support/MacBot";

/// Return the root for all client-local, non-keychain data.
///
/// A configured override must be absolute so a QA run cannot accidentally
/// resolve its data relative to an arbitrary process working directory.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(DATA_DIR_ENV)
        && !raw.is_empty()
    {
        let path = PathBuf::from(raw);
        return checked_data_dir(path);
    }

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(DEFAULT_RELATIVE_DIR))
}

fn checked_data_dir(path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(anyhow!("{DATA_DIR_ENV} must be an absolute path"));
    }
    Ok(path)
}

/// Return a file below the shared client data root.
pub fn data_file(name: &str) -> Result<PathBuf> {
    validate_component(name)?;
    Ok(data_dir()?.join(name))
}

/// Return a subdirectory below the shared client data root.
pub fn data_subdir(name: &str) -> Result<PathBuf> {
    validate_component(name)?;
    Ok(data_dir()?.join(name))
}

fn validate_component(component: &str) -> Result<()> {
    let path = Path::new(component);
    if component.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || path.file_name().and_then(|name| name.to_str()) != Some(component)
    {
        return Err(anyhow!("invalid client data path component"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_override() {
        assert!(checked_data_dir(PathBuf::from("relative/path")).is_err());
        assert_eq!(
            checked_data_dir(PathBuf::from("/tmp/macbot-qa")).unwrap(),
            PathBuf::from("/tmp/macbot-qa")
        );
        assert!(validate_component("nested/file").is_err());
    }

    #[test]
    fn accepts_single_path_components() {
        assert!(validate_component("hosts.json").is_ok());
        assert!(validate_component("cache").is_ok());
        assert!(validate_component("nested/file").is_err());
    }
}
