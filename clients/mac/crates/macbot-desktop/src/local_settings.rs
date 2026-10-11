//! Local desktop preferences that do not belong to the server Settings object.
//!
//! The file is deliberately kept outside the repository and written with
//! private permissions.  Launch-at-login only writes the user's own
//! LaunchAgent; it never calls `launchctl` or starts the application now.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const STORE_FILE: &str = "local-settings.json";
const TRANSLUCENCY_FILE: &str = "local-translucency.json";
const SOUND_FILE: &str = "local-sound.json";
const NOTIFICATION_PREFERENCES_FILE: &str = "local-notification-preferences.json";
const LAUNCH_AGENT_DIR: &str = "Library/LaunchAgents";
const LAUNCH_AGENT_FILE: &str = "bot.mac.desktop.plist";
const BUNDLE_LABEL: &str = "bot.mac.desktop";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LocalSettings {
    pub theme: String,
    pub notifications: bool,
    pub launch_at_login: bool,
}

/// Per-category notification choices are stored separately to keep existing
/// LocalSettings snapshots backward compatible.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct NotificationPreferences {
    pub need_you: bool,
    pub review: bool,
    pub dm_mention: bool,
    pub all_group: bool,
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            need_you: true,
            review: true,
            dm_mention: true,
            all_group: false,
        }
    }
}

pub fn notification_preferences() -> Result<NotificationPreferences> {
    let path = crate::storage_paths::data_file(NOTIFICATION_PREFERENCES_FILE)?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("decode notification preferences {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(NotificationPreferences::default())
        }
        Err(error) => {
            Err(error).with_context(|| format!("read notification preferences {}", path.display()))
        }
    }
}

pub fn set_notification_preference(key: &str, enabled: bool) -> Result<NotificationPreferences> {
    let mut preferences = notification_preferences()?;
    match key {
        "notify_need_you" => preferences.need_you = enabled,
        "notify_review" => preferences.review = enabled,
        "notify_dm_mention" => preferences.dm_mention = enabled,
        "notify_all_group" => preferences.all_group = enabled,
        _ => return Err(anyhow!("unknown notification preference key {key}")),
    }
    let path = crate::storage_paths::data_file(NOTIFICATION_PREFERENCES_FILE)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("notification preferences have no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create local settings directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;
    let bytes =
        serde_json::to_vec_pretty(&preferences).context("encode notification preferences")?;
    atomic_private_write(&path, &bytes)?;
    Ok(preferences)
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            notifications: false,
            launch_at_login: false,
        }
    }
}

pub fn load() -> Result<LocalSettings> {
    load_from_path(&default_store_path()?)
}

pub fn save(settings: &LocalSettings) -> Result<()> {
    save_to_path(&default_store_path()?, settings)
}

/// Whether native window vibrancy is enabled. This preference is stored
/// separately so older `LocalSettings` snapshots remain readable.
pub fn translucent() -> bool {
    load_translucent().unwrap_or(true)
}

/// macOS exposes this accessibility preference through Core Foundation's
/// global preference domain. If it is enabled, use the opaque token set and
/// tell GPUI not to request a vibrancy material.
pub fn reduce_transparency_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        use core_foundation::{
            base::TCFType,
            boolean::{CFBoolean, CFBooleanRef},
            string::{CFString, CFStringRef},
        };
        use std::ffi::c_void;

        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFPreferencesCopyAppValue(
                key: CFStringRef,
                application_id: CFStringRef,
            ) -> *const c_void;
        }

        let key = CFString::new("AppleReduceTransparency");
        let domain = CFString::new("Apple Global Domain");
        let value = unsafe {
            CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), domain.as_concrete_TypeRef())
        };
        if value.is_null() {
            return false;
        }
        let boolean = unsafe { CFBoolean::wrap_under_create_rule(value as CFBooleanRef) };
        bool::from(boolean)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn set_translucent(enabled: bool) -> Result<()> {
    let path = crate::storage_paths::data_file(TRANSLUCENCY_FILE)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("local translucency setting has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create local settings directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;
    atomic_private_write(&path, if enabled { b"true" } else { b"false" })
}

pub fn sound_enabled() -> bool {
    load_sound().unwrap_or(true)
}

pub fn set_sound_enabled(enabled: bool) -> Result<()> {
    let path = crate::storage_paths::data_file(SOUND_FILE)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("local sound setting has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create local settings directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;
    atomic_private_write(&path, if enabled { b"true" } else { b"false" })
}

fn load_translucent() -> Result<bool> {
    let path = crate::storage_paths::data_file(TRANSLUCENCY_FILE)?;
    match fs::read_to_string(&path) {
        Ok(value) => serde_json::from_str(&value)
            .with_context(|| format!("decode translucency setting {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => {
            Err(error).with_context(|| format!("read translucency setting {}", path.display()))
        }
    }
}

fn load_sound() -> Result<bool> {
    let path = crate::storage_paths::data_file(SOUND_FILE)?;
    match fs::read_to_string(&path) {
        Ok(value) => serde_json::from_str(&value)
            .with_context(|| format!("decode sound setting {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error).with_context(|| format!("read sound setting {}", path.display())),
    }
}

pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    let executable = std::env::current_exe().context("resolve current MacBot executable")?;
    set_launch_at_login_at(enabled, Path::new(&home), &executable)
}

fn default_store_path() -> Result<PathBuf> {
    crate::storage_paths::data_file(STORE_FILE)
}

fn load_from_path(path: &Path) -> Result<LocalSettings> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("decode local settings {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LocalSettings::default()),
        Err(error) => Err(error).with_context(|| format!("read local settings {}", path.display())),
    }
}

fn save_to_path(path: &Path, settings: &LocalSettings) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("local settings has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create local settings directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;
    let bytes = serde_json::to_vec_pretty(settings).context("encode local settings")?;
    atomic_private_write(path, &bytes)
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("private file has no parent directory"))?;
    let unique = format!("{}.{}", std::process::id(), unique_suffix());
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("settings"),
        unique
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .with_context(|| format!("create temporary private file {}", temp.display()))?;
        file.write_all(bytes).context("write private file")?;
        file.sync_all().context("sync private file")?;
        set_mode(&temp, 0o600)?;
        fs::rename(&temp, path)
            .with_context(|| format!("replace private file {}", path.display()))?;
        set_mode(path, 0o600)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn launch_agent_path(home: &Path) -> PathBuf {
    home.join(LAUNCH_AGENT_DIR).join(LAUNCH_AGENT_FILE)
}

fn set_launch_at_login_at(enabled: bool, home: &Path, executable: &Path) -> Result<()> {
    let path = launch_agent_path(home);
    if !enabled {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove LaunchAgent {}", path.display()));
            }
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("LaunchAgent has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create LaunchAgent directory {}", parent.display()))?;
    set_mode(parent, 0o700)?;
    atomic_private_write(&path, launch_agent_plist(executable).as_bytes())
}

fn launch_agent_plist(executable: &Path) -> String {
    let executable = xml_escape(&executable.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>{BUNDLE_LABEL}</string><key>ProgramArguments</key><array><string>{executable}</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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

fn unique_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_settings_round_trip_is_private() {
        let root = std::env::temp_dir().join(format!("macbot-local-settings-{}", unique_suffix()));
        let path = root.join("Library/Application Support/MacBot/local-settings.json");
        let settings = LocalSettings {
            theme: "dark".into(),
            notifications: false,
            launch_at_login: true,
        };
        save_to_path(&path, &settings).unwrap();
        assert_eq!(load_from_path(&path).unwrap(), settings);
        assert_eq!(
            fs::metadata(root.join("Library/Application Support/MacBot"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn launch_agent_is_written_and_removed_only_in_test_home() {
        let root = std::env::temp_dir().join(format!("macbot-launch-agent-{}", unique_suffix()));
        let executable = Path::new("/tmp/MacBot.app/Contents/MacOS/macbot-desktop");
        set_launch_at_login_at(true, &root, executable).unwrap();
        let path = launch_agent_path(&root);
        let plist = fs::read_to_string(&path).unwrap();
        assert!(plist.contains("<string>bot.mac.desktop</string>"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist.contains("MacBot.app/Contents/MacOS/macbot-desktop"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        set_launch_at_login_at(false, &root, executable).unwrap();
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }
}
