//! Asynchronous, read-only update discovery and verified download helpers.
//!
//! The helper never installs an application and never restarts the client. It
//! only checks a manifest and downloads a verified artifact to the macOS cache;
//! the UI owns the user-confirmed replacement flow.

use std::{
    cmp::Ordering,
    env,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};

const UPDATE_URL_ENV: &str = "MACBOT_UPDATE_URL";
const BUNDLE_ID: &str = "bot.mac.desktop";
const UPDATE_CONFIG_FILE: &str = "update.json";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct UpdateConfig {
    manifest_url: String,
}

/// A release entry returned by the update manifest.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// A verified artifact and, when the client itself runs from an app bundle,
/// the reviewable installer script that can be launched after confirmation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedUpdate {
    pub artifact: PathBuf,
    pub installer: Option<PathBuf>,
    pub app_path: Option<PathBuf>,
    pub version: String,
}

#[derive(Clone, Debug)]
pub struct UpdateClient {
    client: Client,
    current_version: Version,
    manifest_url: Url,
    cache_dir: PathBuf,
}

impl UpdateClient {
    /// Read `MACBOT_UPDATE_URL`; no URL means update checks are disabled.
    pub fn from_env(current_version: &str) -> Result<Option<Self>> {
        let raw_url = if let Some(raw_url) = env::var_os(UPDATE_URL_ENV) {
            raw_url.to_string_lossy().into_owned()
        } else if let Some(raw_url) = load_update_url()? {
            raw_url
        } else {
            return Ok(None);
        };
        let manifest_url = parse_allowed_url(&raw_url).context("invalid MACBOT_UPDATE_URL")?;
        Ok(Some(Self::new(current_version, manifest_url)?))
    }

    pub fn new(current_version: &str, manifest_url: Url) -> Result<Self> {
        let manifest_url = parse_allowed_url(manifest_url.as_str())?;
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(45))
            .user_agent(concat!("MacBot/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("create update HTTP client")?;
        Ok(Self {
            client,
            current_version: Version::parse(current_version)
                .context("invalid current app version")?,
            manifest_url,
            cache_dir: default_cache_dir()?,
        })
    }

    /// Return a newer release, if one is available.
    pub async fn check(&self) -> Result<Option<Release>> {
        let response = self
            .client
            .get(self.manifest_url.clone())
            .send()
            .await?
            .error_for_status()?;
        ensure_allowed_url(response.url())?;
        let release = response
            .json::<Release>()
            .await
            .context("decode update manifest")?;
        let release_version =
            Version::parse(&release.version).context("invalid release version")?;
        validate_sha256(&release.sha256)?;
        parse_allowed_url(&release.url).context("invalid release URL")?;
        if release_version > self.current_version {
            Ok(Some(release))
        } else {
            Ok(None)
        }
    }

    /// Download a release to the cache and verify its SHA-256 digest.
    ///
    /// The returned path is ready for a caller-owned install/replacement flow.
    /// No application process is started or replaced here.
    pub async fn download(&self, release: &Release) -> Result<PathBuf> {
        let url = parse_allowed_url(&release.url).context("invalid release URL")?;
        let expected = decode_sha256(&release.sha256)?;
        let version = Version::parse(&release.version).context("invalid release version")?;
        fs::create_dir_all(&self.cache_dir)
            .await
            .with_context(|| format!("create update cache {}", self.cache_dir.display()))?;

        let extension = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .and_then(|name| {
                Path::new(name)
                    .extension()
                    .and_then(|extension| extension.to_str())
            })
            .map(|extension| format!(".{extension}"))
            .unwrap_or_else(|| ".download".to_owned());
        let hash_prefix = &release.sha256.trim()[..12];
        let destination = self.cache_dir.join(format!(
            "macbot-{}-{hash_prefix}{extension}",
            version.display()
        ));
        if let Ok(bytes) = fs::read(&destination).await
            && sha256(&bytes) == expected
        {
            return Ok(destination);
        }

        let response = self.client.get(url).send().await?.error_for_status()?;
        ensure_allowed_url(response.url())?;
        let temp = self.cache_dir.join(format!(
            ".{}.{}.tmp",
            destination.file_name().unwrap().to_string_lossy(),
            unique_suffix()
        ));
        let result = async {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .await
                .with_context(|| format!("create update temporary file {}", temp.display()))?;
            let mut hasher = Sha256::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await? {
                hasher.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .context("write update artifact")?;
            }
            file.sync_all().await.context("sync update artifact")?;
            let digest = hasher.finalize();
            if digest.as_slice() != expected.as_slice() {
                return Err(anyhow!("update SHA-256 mismatch"));
            }
            fs::set_permissions(&temp, std::os::unix::fs::PermissionsExt::from_mode(0o600))
                .await
                .context("set update artifact permissions")?;
            fs::rename(&temp, &destination)
                .await
                .with_context(|| format!("commit update artifact {}", destination.display()))?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temp).await;
        }
        result.map(|()| destination)
    }

    /// Download and verify a release, then write an installer script beside it.
    ///
    /// A process launched from a bare Rust binary cannot be replaced safely:
    /// in that case `installer` is `None` and the caller should open the
    /// verified DMG manually.  No mount, replacement, or process launch is
    /// performed by this method.
    pub async fn download_and_stage(&self, release: &Release) -> Result<StagedUpdate> {
        let artifact = self.download(release).await?;
        let app_path = current_app_path();
        let installer = app_path
            .as_deref()
            .map(|app| self.install(&artifact, release, app, std::process::id()))
            .transpose()?;
        Ok(StagedUpdate {
            artifact,
            installer,
            app_path,
            version: release.version.clone(),
        })
    }

    /// Write a self-contained, reviewable installer script for a verified DMG.
    ///
    /// The script rechecks SHA-256, mounts read-only with `hdiutil`, validates
    /// the bundle identifier and version, waits for the old process, copies to
    /// a sibling staging path with `ditto`, then atomically swaps the app.  A
    /// failed swap restores the old app; the backup is intentionally retained.
    /// The caller owns the final user-confirmed execution of this script.
    pub fn install(
        &self,
        artifact: &Path,
        release: &Release,
        app_path: &Path,
        old_pid: u32,
    ) -> Result<PathBuf> {
        if !is_app_bundle(app_path) {
            return Err(anyhow!(
                "current executable is not inside a .app bundle; download the DMG and open it manually"
            ));
        }
        if artifact
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("dmg"))
            != Some(true)
        {
            return Err(anyhow!("verified update artifact is not a DMG"));
        }
        validate_sha256(&release.sha256)?;
        Version::parse(&release.version).context("invalid release version")?;
        let script = self.cache_dir.join(format!(
            "install-{}.sh",
            safe_file_component(&release.version)
        ));
        let target_app = preferred_target_path(app_path);
        let script_text = installer_script(
            artifact,
            &release.sha256,
            &release.version,
            &target_app,
            old_pid,
        );
        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("create update cache {}", self.cache_dir.display()))?;
        std::fs::write(&script, script_text)
            .with_context(|| format!("write update installer {}", script.display()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("set update installer permissions {}", script.display()))?;
        Ok(script)
    }
}

pub fn validate_update_url(raw: &str) -> Result<Url> {
    parse_allowed_url(raw)
}

/// Convenience entry point for callers that only need a one-shot check.
pub async fn check_for_update(current_version: &str) -> Result<Option<Release>> {
    let Some(client) = UpdateClient::from_env(current_version)? else {
        return Ok(None);
    };
    client.check().await
}

/// Load the user-selected update manifest URL from the local app preferences.
/// Environment configuration takes precedence when `UpdateClient::from_env`
/// is used, which keeps deployment and test overrides straightforward.
pub fn load_update_url() -> Result<Option<String>> {
    let path = update_config_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let config: UpdateConfig = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read update config {}", path.display()))?,
    )
    .with_context(|| format!("decode update config {}", path.display()))?;
    let value = config.manifest_url.trim();
    if value.is_empty() {
        return Ok(None);
    }
    validate_update_url(value)?;
    Ok(Some(value.to_owned()))
}

/// Persist a user-selected update manifest URL atomically with mode 0600.
/// Passing an empty string clears the local preference.
pub fn save_update_url(raw: &str) -> Result<()> {
    let value = raw.trim();
    let path = update_config_path()?;
    if value.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("remove update config {}", path.display()))?;
        }
        return Ok(());
    }
    validate_update_url(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("update config has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create update config directory {}", parent.display()))?;
    let temporary = parent.join(format!(".{}.{}.tmp", UPDATE_CONFIG_FILE, unique_suffix()));
    let bytes = serde_json::to_vec_pretty(&UpdateConfig {
        manifest_url: value.to_owned(),
    })?;
    std::fs::write(&temporary, bytes)
        .with_context(|| format!("write update config {}", temporary.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set update config permissions {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("commit update config {}", path.display()));
    }
    Ok(())
}

fn update_config_path() -> Result<PathBuf> {
    crate::storage_paths::data_file(UPDATE_CONFIG_FILE)
}

/// Resolve the currently running bundle. `None` means the app was launched as
/// a raw Rust executable and cannot be replaced by the installer helper.
pub fn current_app_path() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    executable
        .ancestors()
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

/// Read the version embedded in the running `.app` bundle.  Packagers may
/// stamp a version that differs from `CARGO_PKG_VERSION`, so update checks
/// should use this value whenever the client is running from a bundle.
pub fn current_app_version() -> Result<Option<String>> {
    let Some(app) = current_app_path() else {
        return Ok(None);
    };
    Ok(Some(bundle_plist_value(
        &app,
        "CFBundleShortVersionString",
    )?))
}

/// Return the running app version, falling back to the Cargo package version
/// for development binaries launched outside an `.app` bundle.
pub fn current_version() -> String {
    current_app_version()
        .ok()
        .flatten()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

fn bundle_plist_value(app: &Path, key: &str) -> Result<String> {
    let plist = app.join("Contents/Info.plist");
    let command = format!("Print :{key}");
    let plist_text = plist.to_string_lossy();
    let output = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", command.as_str(), plist_text.as_ref()])
        .output()
        .with_context(|| format!("read {} from {}", key, plist.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "PlistBuddy could not read {} from {}",
            key,
            plist.display()
        ));
    }
    let value = String::from_utf8(output.stdout)
        .context("bundle plist value is not valid UTF-8")?
        .trim()
        .to_owned();
    if value.is_empty() {
        return Err(anyhow!("bundle plist value {} is empty", key));
    }
    Ok(value)
}

fn is_app_bundle(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("app")
        && path.join("Contents/Info.plist").is_file()
}

fn preferred_target_path(current_app: &Path) -> PathBuf {
    let home = env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = home.as_deref() {
        if current_app.starts_with(home) {
            return current_app.to_path_buf();
        }
        if let Some(name) = current_app.file_name() {
            return home.join("Applications").join(name);
        }
    }
    current_app.to_path_buf()
}

fn safe_file_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn shell_quote(value: &Path) -> String {
    shell_quote_str(&value.to_string_lossy())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn installer_script(
    artifact: &Path,
    digest: &str,
    version: &str,
    app_path: &Path,
    old_pid: u32,
) -> String {
    let artifact = shell_quote(artifact);
    let app_path = shell_quote(app_path);
    let digest = shell_quote_str(&digest.trim().to_ascii_lowercase());
    let version = shell_quote_str(version.trim());
    format!(
        r#"#!/bin/zsh
set -euo pipefail

ARTIFACT={artifact}
EXPECTED_SHA256={digest}
EXPECTED_VERSION={version}
EXPECTED_BUNDLE_ID={bundle_id}
TARGET_APP={app_path}
OLD_PID={old_pid}
MOUNT_POINT=""
STAGING_APP="${{TARGET_APP:h}}/.MacBot.app.install.$$"
BACKUP_APP="${{TARGET_APP:h}}/.MacBot.app.backup-${{EXPECTED_VERSION//[^A-Za-z0-9._-]/_}}.$$"
BACKUP_CREATED=0

cleanup() {{
  if [[ -n "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" -force >/dev/null 2>&1 || true
  fi
}}
rollback() {{
  if (( BACKUP_CREATED )); then
    if [[ -e "$TARGET_APP" ]]; then
      rm -rf "$TARGET_APP"
    fi
    if [[ -e "$BACKUP_APP" ]]; then
      mv "$BACKUP_APP" "$TARGET_APP"
    fi
  else
    rm -rf "$STAGING_APP"
  fi
}}
on_exit() {{
  install_exit_code=$?
  if (( install_exit_code != 0 )); then rollback || true; fi
  cleanup
  exit $install_exit_code
}}
trap on_exit EXIT

[[ -f "$ARTIFACT" ]] || {{ print -u2 "Update artifact is missing: $ARTIFACT"; exit 1; }}
ACTUAL_SHA256="$(shasum -a 256 "$ARTIFACT" | awk '{{print $1}}')"
[[ "$ACTUAL_SHA256" == "$EXPECTED_SHA256" ]] || {{ print -u2 "Update SHA-256 mismatch"; exit 1; }}

ATTACH_PLIST="$(mktemp "${{TMPDIR:-/tmp}}/macbot-attach.XXXXXX.plist")"
hdiutil attach -readonly -nobrowse -plist "$ARTIFACT" > "$ATTACH_PLIST"
for index in {{0..15}}; do
  candidate="$(plutil -extract "system-entities.$index.mount-point" raw -o - "$ATTACH_PLIST" 2>/dev/null || true)"
  if [[ "$candidate" == /Volumes/* ]]; then MOUNT_POINT="$candidate"; break; fi
done
rm -f "$ATTACH_PLIST"
[[ -n "$MOUNT_POINT" ]] || {{ print -u2 "Unable to locate mounted update volume"; exit 1; }}
SOURCE_APP="$(find "$MOUNT_POINT" -type d -name 'MacBot.app' -prune -print -quit)"
[[ -n "$SOURCE_APP" ]] || {{ print -u2 "MacBot.app is missing from the DMG"; exit 1; }}
PLIST="$SOURCE_APP/Contents/Info.plist"
BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PLIST" 2>/dev/null || true)"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PLIST" 2>/dev/null || true)"
[[ "$BUNDLE_ID" == "$EXPECTED_BUNDLE_ID" ]] || {{ print -u2 "Unexpected bundle identifier: $BUNDLE_ID"; exit 1; }}
[[ "$VERSION" == "$EXPECTED_VERSION" ]] || {{ print -u2 "Unexpected bundle version: $VERSION"; exit 1; }}

if (( OLD_PID > 0 )); then
  while kill -0 "$OLD_PID" 2>/dev/null; do sleep 1; done
fi
mkdir -p "${{TARGET_APP:h}}"
rm -rf "$STAGING_APP"
ditto "$SOURCE_APP" "$STAGING_APP"
if [[ -e "$TARGET_APP" ]]; then
  mv "$TARGET_APP" "$BACKUP_APP"
  BACKUP_CREATED=1
fi
if ! mv "$STAGING_APP" "$TARGET_APP"; then rollback; exit 1; fi
trap - EXIT
cleanup
if [[ "${{MACBOT_TEST_UPDATE_NO_OPEN:-0}}" == "1" ]]; then
  print "Test mode: installed MacBot $EXPECTED_VERSION without opening the app"
else
  open_args=()
  if [[ -n "${{MACBOT_CLIENT_DATA_DIR:-}}" ]]; then
    open_args+=(--env "MACBOT_CLIENT_DATA_DIR=$MACBOT_CLIENT_DATA_DIR")
  fi
  if [[ -n "${{MACBOT_SECRET_BACKEND:-}}" ]]; then
    open_args+=(--env "MACBOT_SECRET_BACKEND=$MACBOT_SECRET_BACKEND")
  fi
  open "${{open_args[@]}}" "$TARGET_APP"
fi
print "Installed MacBot $EXPECTED_VERSION; backup retained at $BACKUP_APP"
"#,
        bundle_id = shell_quote_str(BUNDLE_ID),
    )
}

fn parse_allowed_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).with_context(|| format!("invalid update URL {raw:?}"))?;
    ensure_allowed_url(&url)?;
    Ok(url)
}

fn ensure_allowed_url(url: &Url) -> Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_localhost(url) => Ok(()),
        scheme => Err(anyhow!(
            "update URL must use https (local http is allowed), got {scheme:?}"
        )),
    }
}

fn is_localhost(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn validate_sha256(raw: &str) -> Result<()> {
    decode_sha256(raw).map(|_| ())
}

fn decode_sha256(raw: &str) -> Result<[u8; 32]> {
    let raw = raw.trim();
    if raw.len() != 64 {
        return Err(anyhow!("sha256 must contain 64 hexadecimal characters"));
    }
    let mut output = [0u8; 32];
    for (index, pair) in raw.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Ok(output)
}

fn hex_digit(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(anyhow!("sha256 contains non-hexadecimal characters")),
    }
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut output = [0u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn default_cache_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("MACBOT_UPDATE_CACHE") {
        return Ok(PathBuf::from(path));
    }
    crate::storage_paths::data_subdir("updates")
}

fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Identifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

impl Version {
    fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim().strip_prefix('v').unwrap_or(raw);
        let (core, pre) = raw
            .split_once('-')
            .map_or((raw, None), |(core, pre)| (core, Some(pre)));
        let core = core.split('+').next().unwrap_or(core);
        let mut parts = core.split('.');
        let parse_part = |part: Option<&str>, label: &str| -> Result<u64> {
            let part = part.ok_or_else(|| anyhow!("semver {raw:?} is missing {label}"))?;
            if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
                return Err(anyhow!("invalid semver {raw:?}"));
            }
            part.parse()
                .with_context(|| format!("invalid semver {raw:?}"))
        };
        let major = parse_part(parts.next(), "major")?;
        let minor = parse_part(parts.next(), "minor")?;
        let patch = parse_part(parts.next(), "patch")?;
        if parts.next().is_some() {
            return Err(anyhow!("invalid semver {raw:?}"));
        }
        let pre = pre
            .map(|value| {
                value
                    .split('.')
                    .map(|item| {
                        if item.is_empty() || (item.len() > 1 && item.starts_with('0')) {
                            return Err(anyhow!("invalid semver pre-release {raw:?}"));
                        }
                        Ok(item
                            .parse::<u64>()
                            .map(Identifier::Numeric)
                            .unwrap_or_else(|_| Identifier::Text(item.to_owned())))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    fn display(&self) -> String {
        let core = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if self.pre.is_empty() {
            core
        } else {
            format!(
                "{core}-{}",
                self.pre
                    .iter()
                    .map(|id| match id {
                        Identifier::Numeric(value) => value.to_string(),
                        Identifier::Text(value) => value.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(".")
            )
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_pre(left: &[Identifier], right: &[Identifier]) -> Ordering {
    for (a, b) in left.iter().zip(right.iter()) {
        let ordering = match (a, b) {
            (Identifier::Numeric(a), Identifier::Numeric(b)) => a.cmp(b),
            (Identifier::Numeric(_), Identifier::Text(_)) => Ordering::Less,
            (Identifier::Text(_), Identifier::Numeric(_)) => Ordering::Greater,
            (Identifier::Text(a), Identifier::Text(b)) => a.cmp(b),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn allows_https_and_local_http_only() {
        assert!(validate_update_url("https://updates.example.test/latest.json").is_ok());
        assert!(validate_update_url("http://127.0.0.1:7789/latest.json").is_ok());
        assert!(validate_update_url("http://updates.example.test/latest.json").is_err());
    }

    #[test]
    fn compares_semver_with_prerelease_rules() {
        assert!(Version::parse("1.2.4").unwrap() > Version::parse("1.2.3").unwrap());
        assert!(Version::parse("1.2.3").unwrap() > Version::parse("1.2.3-rc.1").unwrap());
        assert!(Version::parse("1.2.3-rc.2").unwrap() > Version::parse("1.2.3-rc.1").unwrap());
    }

    #[test]
    fn decodes_sha256() {
        assert_eq!(decode_sha256(&"00".repeat(32)).unwrap(), [0u8; 32]);
        assert!(decode_sha256("not-a-digest").is_err());
    }

    #[test]
    fn installer_script_is_reviewable_without_touching_an_app() {
        let script = installer_script(
            Path::new("/tmp/Mac Bot/update.dmg"),
            &"ab".repeat(32),
            "1.2.3",
            Path::new("/Users/test/Applications/Mac Bot.app"),
            42,
        );
        assert!(script.starts_with("#!/bin/zsh\nset -euo pipefail"));
        assert!(script.contains("hdiutil attach -readonly -nobrowse -plist"));
        assert!(script.contains("EXPECTED_BUNDLE_ID='bot.mac.desktop'"));
        assert!(script.contains("ditto \"$SOURCE_APP\" \"$STAGING_APP\""));
        assert!(script.contains("mv \"$TARGET_APP\" \"$BACKUP_APP\""));
        assert!(script.contains("BACKUP_CREATED=0"));
        assert!(script.contains("rollback"));
        assert!(script.contains("install_exit_code=$?"));
        assert!(!script.contains("status=$?"));
        assert!(script.contains("MACBOT_TEST_UPDATE_NO_OPEN"));
        assert!(
            script
                .contains("open_args+=(--env \"MACBOT_CLIENT_DATA_DIR=$MACBOT_CLIENT_DATA_DIR\")")
        );
        assert!(
            script.contains("open_args+=(--env \"MACBOT_SECRET_BACKEND=$MACBOT_SECRET_BACKEND\")")
        );
        assert!(script.contains("open \"${open_args[@]}\" \"$TARGET_APP\""));
        assert!(!script.contains("MACBOT_PASSWORD"));
        assert!(!script.contains("MACBOT_HOST"));
    }

    #[test]
    fn isolated_dmg_install_when_requested() {
        let Some(raw_dmg) = std::env::var_os("MACBOT_TEST_UPDATE_DMG") else {
            return;
        };
        let dmg = PathBuf::from(raw_dmg);
        assert!(
            dmg.is_file(),
            "MACBOT_TEST_UPDATE_DMG is not a file: {}",
            dmg.display()
        );

        let root = std::env::temp_dir().join(format!("macbot-update-test-{}", unique_suffix()));
        let mount = root.join("mounted");
        std::fs::create_dir_all(&mount).unwrap();
        let attach = std::process::Command::new("hdiutil")
            .args(["attach", "-readonly", "-nobrowse", "-mountpoint"])
            .arg(&mount)
            .arg(&dmg)
            .output()
            .expect("run hdiutil attach for isolated update test");
        assert!(
            attach.status.success(),
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&attach.stderr)
        );

        let source_app = mount.join("MacBot.app");
        let source_metadata = if source_app.is_dir() {
            bundle_plist_value(&source_app, "CFBundleIdentifier").and_then(|bundle_id| {
                bundle_plist_value(&source_app, "CFBundleShortVersionString")
                    .map(|version| (bundle_id, version))
            })
        } else {
            Err(anyhow!("DMG does not contain MacBot.app"))
        };
        let detach = std::process::Command::new("hdiutil")
            .args(["detach", "-force"])
            .arg(&mount)
            .output()
            .expect("run hdiutil detach for isolated update test");
        assert!(
            detach.status.success(),
            "hdiutil detach failed: {}",
            String::from_utf8_lossy(&detach.stderr)
        );
        let (bundle_id, version) = source_metadata.unwrap();
        assert_eq!(bundle_id, BUNDLE_ID);

        let target = root.join("target/MacBot.app");
        write_test_bundle(&target, "0.0.1", "old-binary");
        let digest = digest_string(&std::fs::read(&dmg).unwrap());
        let script = root.join("install.sh");
        std::fs::write(
            &script,
            installer_script(&dmg, &digest, &version, &target, 0),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

        let output = std::process::Command::new("/bin/zsh")
            .arg(&script)
            .env("MACBOT_TEST_UPDATE_NO_OPEN", "1")
            .output()
            .expect("run isolated update installer");
        assert!(
            output.status.success(),
            "installer failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            bundle_plist_value(&target, "CFBundleIdentifier").unwrap(),
            BUNDLE_ID
        );
        assert_eq!(
            bundle_plist_value(&target, "CFBundleShortVersionString").unwrap(),
            version
        );

        let target_parent = target.parent().unwrap();
        let backups = std::fs::read_dir(target_parent)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".MacBot.app.backup-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1, "expected one retained backup");
        assert_eq!(
            std::fs::read_to_string(backups[0].join("Contents/MacOS/macbot-desktop")).unwrap(),
            "old-binary"
        );
        let staging_left = std::fs::read_dir(target_parent)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .any(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".MacBot.app.install."))
            });
        assert!(!staging_left, "installer staging app was not cleaned up");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_test_bundle(app: &Path, version: &str, marker: &str) {
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"
            ),
        )
        .unwrap();
        std::fs::write(contents.join("MacOS/macbot-desktop"), marker).unwrap();
    }

    fn digest_string(bytes: &[u8]) -> String {
        sha256(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
