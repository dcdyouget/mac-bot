//! Agent Skills discovery and management.
//!
//! The registry deliberately keeps the index small: only SKILL.md frontmatter is
//! indexed during a scan.  Full skill text and supporting files are read when a
//! caller asks for them.  This makes it suitable for putting the index in the
//! gateway's long lived state without loading all skills into every prompt.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 1024;

const BUILTIN_SKILLS: &[(&str, &str, &str)] = &[
    (
        "agent-browser",
        "Use the agent-browser sidecar for web pages and browser interaction.",
        "builtin://agent-browser",
    ),
    (
        "macbot-collab",
        "Send self-contained messages to other Bots and hand off work safely.",
        "builtin://macbot-collab",
    ),
    (
        "project-home",
        "Follow the MacBot project and Bot Home directory conventions.",
        "builtin://project-home",
    ),
];

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("skill not found: {0}")]
    NotFound(String),
    #[error("skill already exists: {0}")]
    Conflict(String),
    #[error("builtin skill cannot be modified")]
    BuiltinReadOnly,
    #[error("invalid skill: {0}")]
    Invalid(String),
    #[error("unsafe path: {0}")]
    UnsafePath(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git import failed: {0}")]
    Git(String),
    #[error("archive error: {0}")]
    Archive(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Builtin,
    User,
    Imported,
    Draft,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct InvocationStats {
    pub total: u64,
    pub by_bot: Vec<BotInvocation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotInvocation {
    pub bot_id: String,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotSkillSettingsSnapshot {
    pub bot_id: String,
    pub entries: Vec<BotSkillSettingsEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotSkillSettingsEntry {
    pub name: String,
    pub disabled_bot_ids: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub path: String,
    pub files: Vec<String>,
    pub enabled: bool,
    pub disabled_bot_ids: Vec<String>,
    pub invocations_7d: InvocationStats,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub skill: Skill,
    pub content: String,
}

#[derive(Clone, Debug)]
struct IndexedSkill {
    skill: Skill,
    disable_model_invocation: bool,
}

/// A filesystem-backed skills registry. `roots` are ordered: the first valid
/// skill with a name wins, matching pi's conflict rule.
#[derive(Debug)]
pub struct SkillRegistry {
    roots: Vec<PathBuf>,
    install_root: PathBuf,
    entries: BTreeMap<String, IndexedSkill>,
    root_fingerprints: BTreeMap<PathBuf, u128>,
}

impl SkillRegistry {
    pub fn new(home: impl Into<PathBuf>, extra_dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        let home = home.into();
        let install_root = home.join("skills");
        let mut roots = vec![install_root.clone()];
        roots.extend(extra_dirs);
        let mut registry = Self {
            roots,
            install_root,
            entries: BTreeMap::new(),
            root_fingerprints: BTreeMap::new(),
        };
        registry.add_builtins();
        registry
    }

    pub fn with_roots(install_root: impl Into<PathBuf>, roots: Vec<PathBuf>) -> Self {
        let install_root = install_root.into();
        let mut registry = Self {
            roots,
            install_root,
            entries: BTreeMap::new(),
            root_fingerprints: BTreeMap::new(),
        };
        registry.add_builtins();
        registry
    }

    fn add_builtins(&mut self) {
        let now = Utc::now();
        for (name, description, path) in BUILTIN_SKILLS {
            self.entries.insert(
                (*name).to_string(),
                IndexedSkill {
                    skill: Skill {
                        name: (*name).to_string(),
                        description: (*description).to_string(),
                        source: SkillSource::Builtin,
                        path: (*path).to_string(),
                        files: Vec::new(),
                        enabled: true,
                        disabled_bot_ids: Vec::new(),
                        invocations_7d: InvocationStats::default(),
                        updated_at: now,
                    },
                    disable_model_invocation: false,
                },
            );
        }
    }

    /// Scan all roots. Existing index metadata such as per-Bot disables and
    /// invocation counters is retained when a file is unchanged.
    pub fn rescan(&mut self) -> Result<Vec<Skill>, SkillError> {
        let old = std::mem::take(&mut self.entries);
        self.entries = BTreeMap::new();
        self.add_builtins();
        for root in self.roots.clone() {
            if !root.exists() {
                continue;
            }
            for dir in skill_dirs(&root)? {
                let Some(indexed) = read_skill_dir(&dir)? else {
                    continue;
                };
                if self.entries.contains_key(&indexed.skill.name) {
                    continue;
                }
                let indexed = if let Some(previous) = old.get(&indexed.skill.name) {
                    let mut fresh = indexed;
                    fresh.skill.enabled = previous.skill.enabled;
                    fresh.skill.disabled_bot_ids = previous.skill.disabled_bot_ids.clone();
                    fresh.skill.invocations_7d = previous.skill.invocations_7d.clone();
                    fresh
                } else {
                    indexed
                };
                self.entries.insert(indexed.skill.name.clone(), indexed);
            }
        }
        self.refresh_fingerprints();
        Ok(self.list())
    }

    /// Restore mutable index metadata from the durable feature snapshot after
    /// a scan. File contents and paths are never taken from the snapshot;
    /// only metadata for an entry that was found in an allowed root is used.
    pub fn restore_metadata(&mut self, saved: &[Skill]) {
        for saved_skill in saved {
            let Some(entry) = self.entries.get_mut(&saved_skill.name) else {
                continue;
            };
            if entry.skill.source != saved_skill.source || entry.skill.path != saved_skill.path {
                continue;
            }
            entry.skill.enabled = saved_skill.enabled;
            entry.skill.disabled_bot_ids = saved_skill.disabled_bot_ids.clone();
            entry.skill.invocations_7d = saved_skill.invocations_7d.clone();
            entry.skill.updated_at = saved_skill.updated_at;
        }
    }

    /// Cheap polling hook for a gateway watcher. Call this periodically after
    /// a filesystem notification or timer tick.
    pub fn rescan_if_changed(&mut self) -> Result<Option<Vec<Skill>>, SkillError> {
        let before = self.root_fingerprints.clone();
        self.refresh_fingerprints();
        if before == self.root_fingerprints {
            return Ok(None);
        }
        Ok(Some(self.rescan()?))
    }

    /// Replace settings.extra_dirs and immediately rebuild the frontmatter
    /// index. Full skill bodies remain lazy and are not loaded during scan.
    pub fn set_extra_dirs(
        &mut self,
        extra_dirs: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Vec<Skill>, SkillError> {
        self.roots = std::iter::once(self.install_root.clone())
            .chain(extra_dirs)
            .collect();
        self.rescan()
    }

    fn refresh_fingerprints(&mut self) {
        self.root_fingerprints = self
            .roots
            .iter()
            .map(|root| (root.clone(), directory_fingerprint(root)))
            .collect();
    }

    pub fn list(&self) -> Vec<Skill> {
        self.entries
            .values()
            .map(|entry| entry.skill.clone())
            .collect()
    }

    pub fn get(&self, name: &str) -> Result<SkillDetail, SkillError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        let content = if entry.skill.source == SkillSource::Builtin {
            builtin_content(&entry.skill.name)
        } else {
            fs::read_to_string(skill_file_path(&entry.skill.path))?
        };
        Ok(SkillDetail {
            skill: entry.skill.clone(),
            content,
        })
    }

    /// Load full SKILL.md text only after the caller has checked the Bot's
    /// per-skill enablement. This is the gateway-facing `skill` tool API.
    pub fn load_for_bot(
        &self,
        name: &str,
        bot_id: Option<&str>,
    ) -> Result<SkillDetail, SkillError> {
        if self
            .entries
            .get(name)
            .is_some_and(|entry| entry.skill.source == SkillSource::Draft)
        {
            return Err(SkillError::Invalid(
                "draft skill must be published before invocation".into(),
            ));
        }
        if !self.is_enabled_for(name, bot_id)? {
            return Err(SkillError::Invalid("skill is disabled for this Bot".into()));
        }
        self.get(name)
    }

    /// Resolve the protocol's explicit `/skill-name instruction` shorthand.
    /// Unknown names return `None` so ordinary slash-prefixed user text keeps
    /// its normal meaning.
    pub fn load_for_text(
        &self,
        text: &str,
        bot_id: Option<&str>,
    ) -> Result<Option<(SkillDetail, String)>, SkillError> {
        let Some((name, instruction)) = forced_invocation(text) else {
            return Ok(None);
        };
        if !self.entries.contains_key(name) {
            return Ok(None);
        }
        Ok(Some((
            self.load_for_bot(name, bot_id)?,
            instruction.to_string(),
        )))
    }

    /// Whether the model may choose this skill on its own. Explicit user
    /// `/skill-name` invocations can still call `load_for_bot` when enabled.
    pub fn model_invocation_allowed(&self, name: &str) -> Result<bool, SkillError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        Ok(!entry.disable_model_invocation)
    }

    pub fn create(&mut self, name: &str, content: &str) -> Result<Skill, SkillError> {
        validate_name(name)?;
        if self.entries.contains_key(name) {
            return Err(SkillError::Conflict(name.to_string()));
        }
        let (parsed_name, _) = parse_frontmatter(content)?;
        if parsed_name != name {
            return Err(SkillError::Invalid(format!(
                "frontmatter name {:?} does not match requested name {:?}",
                parsed_name, name
            )));
        }
        let dir = self.install_root.join(name);
        write_skill_dir(&dir, content)?;
        let indexed = make_indexed(dir, SkillSource::User)?;
        let skill = indexed.skill.clone();
        self.entries.insert(name.to_string(), indexed);
        Ok(skill)
    }

    /// Create an unpublished Bot-generated draft under `skills/.drafts`.
    pub fn create_draft(&mut self, name: &str, content: &str) -> Result<Skill, SkillError> {
        validate_name(name)?;
        if self.entries.contains_key(name) {
            return Err(SkillError::Conflict(name.to_string()));
        }
        let (parsed_name, _) = parse_frontmatter(content)?;
        if parsed_name != name {
            return Err(SkillError::Invalid("frontmatter name mismatch".into()));
        }
        let dir = self.install_root.join(".drafts").join(name);
        write_skill_dir(&dir, content)?;
        let indexed = make_indexed(dir, SkillSource::Draft)?;
        let skill = indexed.skill.clone();
        self.entries.insert(name.to_string(), indexed);
        Ok(skill)
    }

    pub fn update(&mut self, name: &str, content: &str) -> Result<Skill, SkillError> {
        let (path, source, enabled, disabled_bot_ids, invocations_7d) = {
            let entry = self
                .entries
                .get(name)
                .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
            (
                entry.skill.path.clone(),
                entry.skill.source.clone(),
                entry.skill.enabled,
                entry.skill.disabled_bot_ids.clone(),
                entry.skill.invocations_7d.clone(),
            )
        };
        if source == SkillSource::Builtin {
            return Err(SkillError::BuiltinReadOnly);
        }
        let (parsed_name, _) = parse_frontmatter(content)?;
        if parsed_name != name {
            return Err(SkillError::Invalid("frontmatter name mismatch".into()));
        }
        write_skill_dir(Path::new(&path), content)?;
        let mut indexed = make_indexed(PathBuf::from(&path), source)?;
        // Updating SKILL.md must not reset mutable registry metadata. These
        // fields are outside frontmatter and are persisted by the feature
        // snapshot, so a content edit cannot re-enable a disabled skill or
        // erase a Bot-specific disablement.
        indexed.skill.enabled = enabled;
        indexed.skill.disabled_bot_ids = disabled_bot_ids;
        indexed.skill.invocations_7d = invocations_7d;
        let skill = indexed.skill.clone();
        self.entries.insert(name.to_string(), indexed);
        Ok(skill)
    }

    pub fn delete(&mut self, name: &str) -> Result<(), SkillError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        if entry.skill.source == SkillSource::Builtin {
            return Err(SkillError::BuiltinReadOnly);
        }
        if entry.skill.source == SkillSource::Builtin
            || !Path::new(&entry.skill.path).starts_with(&self.install_root)
        {
            return Err(SkillError::UnsafePath(entry.skill.path.clone()));
        }
        fs::remove_dir_all(&entry.skill.path)?;
        self.entries.remove(name);
        Ok(())
    }

    pub fn set_enabled(
        &mut self,
        name: &str,
        enabled: bool,
        bot_id: Option<&str>,
    ) -> Result<Skill, SkillError> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        if let Some(bot_id) = bot_id {
            entry.skill.disabled_bot_ids.retain(|id| id != bot_id);
            if !enabled {
                entry.skill.disabled_bot_ids.push(bot_id.to_string());
                entry.skill.disabled_bot_ids.sort();
                entry.skill.disabled_bot_ids.dedup();
            }
        } else {
            entry.skill.enabled = enabled;
        }
        entry.skill.updated_at = Utc::now();
        Ok(entry.skill.clone())
    }

    /// Copy only per-Bot disablement settings when a Bot is duplicated.
    /// Global skill state, invocation counters and content belong to the
    /// shared skill and must not be copied as Bot history.
    pub fn copy_bot_settings(
        &mut self,
        source_bot_id: &str,
        target_bot_id: &str,
    ) -> Result<Vec<Skill>, SkillError> {
        if source_bot_id.is_empty() || target_bot_id.is_empty() {
            return Err(SkillError::Invalid("Bot id is required".into()));
        }
        if source_bot_id == target_bot_id {
            return Err(SkillError::Invalid(
                "source and target Bot must differ".into(),
            ));
        }
        for entry in self.entries.values_mut() {
            let source_disabled = entry
                .skill
                .disabled_bot_ids
                .iter()
                .any(|id| id == source_bot_id);
            let previous = entry.skill.disabled_bot_ids.clone();
            entry
                .skill
                .disabled_bot_ids
                .retain(|id| id != target_bot_id);
            if source_disabled {
                entry.skill.disabled_bot_ids.push(target_bot_id.to_string());
                entry.skill.disabled_bot_ids.sort();
            }
            if entry.skill.disabled_bot_ids != previous {
                entry.skill.updated_at = Utc::now();
            }
        }
        Ok(self.list())
    }

    pub fn snapshot_bot_settings(
        &self,
        bot_id: &str,
    ) -> Result<BotSkillSettingsSnapshot, SkillError> {
        if bot_id.is_empty() {
            return Err(SkillError::Invalid("Bot id is required".into()));
        }
        Ok(BotSkillSettingsSnapshot {
            bot_id: bot_id.to_string(),
            entries: self
                .entries
                .values()
                .map(|entry| BotSkillSettingsEntry {
                    name: entry.skill.name.clone(),
                    disabled_bot_ids: entry.skill.disabled_bot_ids.clone(),
                    updated_at: entry.skill.updated_at,
                })
                .collect(),
        })
    }

    pub fn restore_bot_settings(
        &mut self,
        snapshot: &BotSkillSettingsSnapshot,
    ) -> Result<(), SkillError> {
        if snapshot.bot_id.is_empty() {
            return Err(SkillError::Invalid("Bot id is required".into()));
        }
        for saved in &snapshot.entries {
            let Some(entry) = self.entries.get_mut(&saved.name) else {
                continue;
            };
            let was_disabled = saved
                .disabled_bot_ids
                .iter()
                .any(|id| id == &snapshot.bot_id);
            let is_disabled = entry
                .skill
                .disabled_bot_ids
                .iter()
                .any(|id| id == &snapshot.bot_id);
            if is_disabled == was_disabled {
                continue;
            }
            entry
                .skill
                .disabled_bot_ids
                .retain(|id| id != &snapshot.bot_id);
            if was_disabled {
                entry.skill.disabled_bot_ids.push(snapshot.bot_id.clone());
                entry.skill.disabled_bot_ids.sort();
            }
            // Preserve a concurrent change made for another Bot. The
            // timestamp reflects this target-only rollback when it changes
            // the membership, rather than restoring an obsolete global time.
            entry.skill.updated_at = Utc::now();
        }
        Ok(())
    }

    pub fn is_enabled_for(&self, name: &str, bot_id: Option<&str>) -> Result<bool, SkillError> {
        let skill = &self
            .entries
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?
            .skill;
        Ok(
            skill.enabled
                && bot_id.is_none_or(|id| !skill.disabled_bot_ids.iter().any(|x| x == id)),
        )
    }

    pub fn publish(&mut self, name: &str) -> Result<Skill, SkillError> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?
            .clone();
        if entry.skill.source != SkillSource::Draft {
            return Err(SkillError::Invalid(
                "only draft skills can be published".into(),
            ));
        }
        let old_dir = PathBuf::from(&entry.skill.path);
        let new_dir = self.install_root.join(name);
        if new_dir.exists() {
            return Err(SkillError::Conflict(name.to_string()));
        }
        fs::rename(&old_dir, &new_dir)?;
        let indexed = make_indexed(new_dir, SkillSource::User)?;
        let skill = indexed.skill.clone();
        self.entries.insert(name.to_string(), indexed);
        Ok(skill)
    }

    pub fn import_path(&mut self, source: impl AsRef<Path>) -> Result<Vec<Skill>, SkillError> {
        let source = source.as_ref();
        let source = fs::canonicalize(source)?;
        let dirs = if source.is_file() {
            if source.file_name().and_then(|x| x.to_str()) != Some("SKILL.md") {
                return Err(SkillError::Invalid(
                    "path import must be a skill directory or SKILL.md".into(),
                ));
            }
            vec![source
                .parent()
                .ok_or_else(|| SkillError::Invalid("missing parent".into()))?
                .to_path_buf()]
        } else {
            skill_dirs(&source)?
        };
        let mut result = Vec::new();
        for dir in dirs {
            let Some(parsed) = read_skill_dir(&dir)? else {
                continue;
            };
            if self.entries.contains_key(&parsed.skill.name) {
                continue;
            }
            let destination = self.install_root.join(&parsed.skill.name);
            copy_skill_dir(&dir, &destination)?;
            let indexed = make_indexed(destination, SkillSource::Imported)?;
            result.push(indexed.skill.clone());
            self.entries.insert(indexed.skill.name.clone(), indexed);
        }
        Ok(result)
    }

    pub fn import_git(
        &mut self,
        url: &str,
        subdir: Option<&str>,
    ) -> Result<Vec<Skill>, SkillError> {
        if url.trim().is_empty() || url.starts_with('-') {
            return Err(SkillError::Invalid("invalid git URL".into()));
        }
        let tmp = tempfile::tempdir()?;
        let status = Command::new("git")
            .args(["clone", "--depth", "1", "--", url])
            .arg(tmp.path())
            .status()
            .map_err(|err| SkillError::Git(err.to_string()))?;
        if !status.success() {
            return Err(SkillError::Git(format!("git clone exited with {status}")));
        }
        let root = resolve_git_subdir(tmp.path(), subdir)?;
        self.import_path(root)
    }

    /// Import a zip payload received by the HTTP upload endpoint. The archive
    /// is extracted into a private temporary directory after rejecting traversal,
    /// absolute paths and symlinks, then passed through normal skill validation.
    pub fn import_zip(&mut self, bytes: &[u8]) -> Result<Vec<Skill>, SkillError> {
        let tmp = tempfile::tempdir()?;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|err| SkillError::Archive(err.to_string()))?;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|err| SkillError::Archive(err.to_string()))?;
            let relative = file
                .enclosed_name()
                .ok_or_else(|| SkillError::UnsafePath(file.name().to_string()))?
                .to_path_buf();
            if file.is_symlink()
                || file
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(SkillError::UnsafePath(file.name().to_string()));
            }
            let out = tmp.path().join(&relative);
            if file.is_dir() {
                fs::create_dir_all(&out)?;
            } else {
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                let mut output = fs::File::create(&out)?;
                output.write_all(&data)?;
            }
        }
        self.import_path(tmp.path())
    }

    pub fn record_invocation(
        &mut self,
        name: &str,
        bot_id: Option<&str>,
    ) -> Result<(), SkillError> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| SkillError::NotFound(name.to_string()))?;
        entry.skill.invocations_7d.total += 1;
        if let Some(bot_id) = bot_id {
            if let Some(row) = entry
                .skill
                .invocations_7d
                .by_bot
                .iter_mut()
                .find(|x| x.bot_id == bot_id)
            {
                row.count += 1;
            } else {
                entry.skill.invocations_7d.by_bot.push(BotInvocation {
                    bot_id: bot_id.to_string(),
                    count: 1,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Frontmatter {
    name: String,
    description: String,
    disable_model_invocation: bool,
}

pub fn parse_frontmatter(content: &str) -> Result<(String, String), SkillError> {
    let parsed = parse_frontmatter_inner(content)?;
    Ok((parsed.name, parsed.description))
}

pub fn forced_invocation(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('/')?;
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let name = &text[..end];
    validate_name(name).ok()?;
    let instruction = text[end..].trim_start();
    Some((name, instruction))
}

fn parse_frontmatter_inner(content: &str) -> Result<Frontmatter, SkillError> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(SkillError::Invalid(
            "SKILL.md must start with YAML frontmatter".into(),
        ));
    }
    let mut name = None;
    let mut description = None;
    let mut disable_model_invocation = false;
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']);
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "disable-model-invocation" => {
                disable_model_invocation = value.eq_ignore_ascii_case("true");
            }
            _ => {}
        }
    }
    if !closed {
        return Err(SkillError::Invalid("unterminated YAML frontmatter".into()));
    }
    let name = name.ok_or_else(|| SkillError::Invalid("frontmatter requires name".into()))?;
    let description = description
        .ok_or_else(|| SkillError::Invalid("frontmatter requires description".into()))?;
    validate_name(&name)?;
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(SkillError::Invalid(
            "description exceeds 1024 characters".into(),
        ));
    }
    Ok(Frontmatter {
        name,
        description,
        disable_model_invocation,
    })
}

fn validate_name(name: &str) -> Result<(), SkillError> {
    if name.is_empty()
        || name.chars().count() > MAX_NAME
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(SkillError::Invalid(
            "name must contain only lowercase letters, digits, and hyphens (max 64)".into(),
        ));
    }
    Ok(())
}

fn skill_dirs(root: &Path) -> Result<Vec<PathBuf>, SkillError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || entry.file_name() != "SKILL.md" {
            continue;
        }
        if entry
            .path()
            .components()
            .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
        {
            continue;
        }
        if let Some(parent) = entry.path().parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn read_skill_dir(dir: &Path) -> Result<Option<IndexedSkill>, SkillError> {
    let path = dir.join("SKILL.md");
    if !path.is_file() {
        return Ok(None);
    }
    let parsed = read_frontmatter_file(&path)?;
    let source = if dir
        .components()
        .any(|component| matches!(component, Component::Normal(name) if name == ".drafts"))
    {
        SkillSource::Draft
    } else {
        SkillSource::User
    };
    let indexed = make_indexed(dir.to_path_buf(), source)?;
    // Keep this explicit validation here so malformed files fail the scan
    // before they can shadow an earlier root.
    if indexed.skill.name != parsed.name {
        return Err(SkillError::Invalid("frontmatter name mismatch".into()));
    }
    Ok(Some(indexed))
}

fn make_indexed(dir: PathBuf, source: SkillSource) -> Result<IndexedSkill, SkillError> {
    let parsed = read_frontmatter_file(&dir.join("SKILL.md"))?;
    let mut files = Vec::new();
    if dir.is_dir() {
        for entry in WalkDir::new(&dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() == "SKILL.md" {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&dir)
                .map_err(|_| SkillError::Invalid("invalid skill path".into()))?;
            files.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    files.sort();
    Ok(IndexedSkill {
        skill: Skill {
            name: parsed.name,
            description: parsed.description,
            source,
            path: dir.to_string_lossy().to_string(),
            files,
            enabled: true,
            disabled_bot_ids: Vec::new(),
            invocations_7d: InvocationStats::default(),
            updated_at: Utc::now(),
        },
        disable_model_invocation: parsed.disable_model_invocation,
    })
}

/// Read only the bounded YAML frontmatter during indexing. The body remains
/// on disk and is loaded by `get`/`load_for_bot` when a model actually invokes
/// the skill.
fn read_frontmatter_file(path: &Path) -> Result<Frontmatter, SkillError> {
    const MAX_FRONTMATTER_BYTES: usize = 128 * 1024;
    let file = fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut text = String::new();
    let mut line = String::new();
    let mut saw_open = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        text.push_str(&line);
        if text.len() > MAX_FRONTMATTER_BYTES {
            return Err(SkillError::Invalid(
                "SKILL.md frontmatter exceeds 128 KiB".into(),
            ));
        }
        if text.lines().count() == 1 && line.trim() == "---" {
            saw_open = true;
            continue;
        }
        if saw_open && line.trim() == "---" {
            return parse_frontmatter_inner(&text);
        }
    }
    Err(SkillError::Invalid("unterminated YAML frontmatter".into()))
}

fn skill_file_path(path: &str) -> PathBuf {
    Path::new(path).join("SKILL.md")
}

fn write_skill_dir(dir: &Path, content: &str) -> Result<(), SkillError> {
    ensure_safe_component(dir.file_name().and_then(|x| x.to_str()).unwrap_or_default())?;
    fs::create_dir_all(dir)?;
    fs::write(dir.join("SKILL.md"), content)?;
    Ok(())
}

fn copy_skill_dir(source: &Path, destination: &Path) -> Result<(), SkillError> {
    if destination.exists() {
        return Err(SkillError::Conflict(destination.display().to_string()));
    }
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| SkillError::UnsafePath(entry.path().display().to_string()))?;
        let out = destination.join(relative);
        if entry.file_type().is_symlink() {
            return Err(SkillError::UnsafePath(entry.path().display().to_string()));
        }
        if entry.file_type().is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &out)?;
        }
    }
    Ok(())
}

fn ensure_safe_component(value: &str) -> Result<(), SkillError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(SkillError::UnsafePath(value.to_string()));
    }
    Ok(())
}

fn directory_fingerprint(root: &Path) -> u128 {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok().and_then(|meta| meta.modified().ok()))
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .fold(0u128, |acc, value| acc.wrapping_add(value))
}

fn resolve_git_subdir(root: &Path, subdir: Option<&str>) -> Result<PathBuf, SkillError> {
    let root = fs::canonicalize(root)?;
    let requested = subdir.map_or_else(|| root.clone(), |subdir| root.join(subdir));
    let resolved = fs::canonicalize(requested)?;
    if !resolved.starts_with(&root) {
        return Err(SkillError::UnsafePath(
            subdir.unwrap_or_default().to_string(),
        ));
    }
    Ok(resolved)
}

fn builtin_content(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Built-in MacBot skill.\n---\n\nThis built-in skill is provided by macbotd.\n")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillImport {
    pub kind: String,
    pub source: String,
    pub subdir: Option<String>,
}

pub fn generated_upload_id() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_text(name: &str) -> String {
        format!("---\nname: {name}\ndescription: A useful skill\n---\n\n# {name}\n")
    }

    #[test]
    fn scans_frontmatter_and_loads_body_on_demand() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills").join("demo");
        fs::create_dir_all(root.join("references")).unwrap();
        fs::write(root.join("SKILL.md"), skill_text("demo")).unwrap();
        fs::write(root.join("references/a.txt"), "ref").unwrap();
        let mut registry = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("skills")],
        );
        registry.rescan().unwrap();
        let skill = registry.get("demo").unwrap();
        assert_eq!(skill.skill.files, vec!["references/a.txt"]);
        assert!(skill.content.contains("# demo"));
    }

    #[test]
    fn respects_disable_model_invocation_frontmatter() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills").join("manual");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: manual\ndescription: Manual only\ndisable-model-invocation: true\n---\n",
        )
        .unwrap();
        let mut registry = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("skills")],
        );
        registry.rescan().unwrap();
        assert!(!registry.model_invocation_allowed("manual").unwrap());
        assert_eq!(
            registry.load_for_bot("manual", None).unwrap().skill.name,
            "manual"
        );
    }

    #[test]
    fn forced_skill_invocation_loads_only_known_skill() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills").join("demo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), skill_text("demo")).unwrap();
        let mut registry = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("skills")],
        );
        registry.rescan().unwrap();
        let loaded = registry
            .load_for_text("/demo do the thing", None)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.0.skill.name, "demo");
        assert_eq!(loaded.1, "do the thing");
        assert!(registry
            .load_for_text("/missing ordinary text", None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn first_root_wins_and_bot_disable_is_scoped() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a/demo");
        let b = temp.path().join("b/demo");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("SKILL.md"), skill_text("demo")).unwrap();
        fs::write(b.join("SKILL.md"), skill_text("demo")).unwrap();
        let mut registry = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("a"), temp.path().join("b")],
        );
        registry.rescan().unwrap();
        assert!(registry.get("demo").unwrap().skill.path.ends_with("a/demo"));
        registry.set_enabled("demo", false, Some("bot-1")).unwrap();
        assert!(!registry.is_enabled_for("demo", Some("bot-1")).unwrap());
        assert!(registry.is_enabled_for("demo", Some("bot-2")).unwrap());
    }

    #[test]
    fn duplicate_copies_only_source_bot_disablement() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        registry.create("demo", &skill_text("demo")).unwrap();
        registry
            .set_enabled("demo", false, Some("source-bot"))
            .unwrap();
        registry
            .record_invocation("demo", Some("source-bot"))
            .unwrap();
        registry
            .copy_bot_settings("source-bot", "target-bot")
            .unwrap();
        assert!(!registry.is_enabled_for("demo", Some("target-bot")).unwrap());
        assert_eq!(registry.get("demo").unwrap().skill.invocations_7d.total, 1);
        assert!(registry
            .copy_bot_settings("target-bot", "target-bot")
            .is_err());
    }

    #[test]
    fn bot_skill_settings_snapshot_restores_target_state() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        registry.create("demo", &skill_text("demo")).unwrap();
        let snapshot = registry.snapshot_bot_settings("target-bot").unwrap();
        registry
            .set_enabled("demo", false, Some("target-bot"))
            .unwrap();
        registry.restore_bot_settings(&snapshot).unwrap();
        assert!(registry.is_enabled_for("demo", Some("target-bot")).unwrap());
    }

    #[test]
    fn restoring_target_settings_preserves_concurrent_other_bot_changes() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        registry.create("demo", &skill_text("demo")).unwrap();
        registry
            .set_enabled("demo", false, Some("source-bot"))
            .unwrap();
        let snapshot = registry.snapshot_bot_settings("target-bot").unwrap();
        registry
            .copy_bot_settings("source-bot", "target-bot")
            .unwrap();
        registry
            .set_enabled("demo", false, Some("other-bot"))
            .unwrap();
        registry.restore_bot_settings(&snapshot).unwrap();
        assert!(registry.is_enabled_for("demo", Some("target-bot")).unwrap());
        assert!(!registry.is_enabled_for("demo", Some("other-bot")).unwrap());
        assert!(!registry.is_enabled_for("demo", Some("source-bot")).unwrap());
    }

    #[test]
    fn rejects_zip_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut bytes);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("../../evil", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        assert!(matches!(
            registry.import_zip(bytes.get_ref()),
            Err(SkillError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_filesystem_symlink_entries() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source/demo");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("SKILL.md"), skill_text("demo")).unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside"), source.join("link")).unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        assert!(matches!(
            registry.import_path(temp.path().join("source")),
            Err(SkillError::UnsafePath(_))
        ));
    }

    #[test]
    fn rejects_git_subdir_escape_and_builtin_writes() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("safe")).unwrap();
        assert!(matches!(
            resolve_git_subdir(temp.path(), Some("../")),
            Err(SkillError::UnsafePath(_))
        ));
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        let content = skill_text("agent-browser");
        assert!(matches!(
            registry.update("agent-browser", &content),
            Err(SkillError::BuiltinReadOnly)
        ));
        assert!(matches!(
            registry.delete("agent-browser"),
            Err(SkillError::BuiltinReadOnly)
        ));
    }

    #[test]
    fn manages_draft_publish() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("skills"), vec![]);
        registry
            .create_draft("draft", &skill_text("draft"))
            .unwrap();
        assert_eq!(
            registry.get("draft").unwrap().skill.source,
            SkillSource::Draft
        );
        let published = registry.publish("draft").unwrap();
        assert_eq!(published.source, SkillSource::User);
    }

    #[test]
    fn draft_cannot_be_invoked_before_publish() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("skills"), vec![]);
        registry
            .create_draft("draft", &skill_text("draft"))
            .unwrap();
        assert!(matches!(
            registry.load_for_bot("draft", None),
            Err(SkillError::Invalid(message)) if message.contains("published")
        ));
        registry.publish("draft").unwrap();
        assert!(registry.load_for_bot("draft", None).is_ok());
    }

    #[test]
    fn extra_dirs_reload_and_filesystem_changes_are_detected() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(first.join("one")).unwrap();
        fs::write(first.join("one/SKILL.md"), skill_text("one")).unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        registry.set_extra_dirs(vec![first.clone()]).unwrap();
        assert!(registry.get("one").is_ok());
        fs::create_dir_all(second.join("two")).unwrap();
        fs::write(second.join("two/SKILL.md"), skill_text("two")).unwrap();
        registry.set_extra_dirs(vec![second]).unwrap();
        assert!(registry.get("two").is_ok());
        assert!(registry.get("one").is_err());
    }

    #[test]
    fn restores_enablement_and_invocations_after_rescan() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("external/demo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), skill_text("demo")).unwrap();
        let mut registry = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("external")],
        );
        registry.rescan().unwrap();
        registry.set_enabled("demo", false, Some("bot-a")).unwrap();
        registry.record_invocation("demo", Some("bot-a")).unwrap();
        let saved = registry.list();

        let mut restored = SkillRegistry::with_roots(
            temp.path().join("install"),
            vec![temp.path().join("external")],
        );
        restored.rescan().unwrap();
        restored.restore_metadata(&saved);
        let skill = restored.get("demo").unwrap().skill;
        assert!(!restored.is_enabled_for("demo", Some("bot-a")).unwrap());
        assert_eq!(skill.invocations_7d.total, 1);
        assert_eq!(skill.disabled_bot_ids, vec!["bot-a"]);
    }

    #[test]
    fn update_preserves_global_and_bot_enablement_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let mut registry = SkillRegistry::with_roots(temp.path().join("install"), vec![]);
        registry.create("demo", &skill_text("demo")).unwrap();
        registry.set_enabled("demo", false, None).unwrap();
        registry.set_enabled("demo", false, Some("bot-a")).unwrap();

        let updated = registry
            .update(
                "demo",
                "---\nname: demo\ndescription: updated\n---\n# updated\n",
            )
            .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.disabled_bot_ids, vec!["bot-a"]);
        assert!(!registry.is_enabled_for("demo", None).unwrap());
        assert!(!registry.is_enabled_for("demo", Some("bot-a")).unwrap());
    }
}
