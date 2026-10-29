//! What this host installed from the depot, persisted as one JSON file
//! (`<modules.storage.install_dir>/runtime/depot/state.json`), and the
//! backups that undo an install (`.../depot/backups/<name>/`).
//!
//! Every write goes to a sibling `*.tmp`, is synced and renamed over the
//! target, so a crash leaves either the old or the new file, never half of
//! one (the pattern of `persist_service_snapshot`).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::domain::module::ModuleManifest;
use crate::utils::system_time_to_rfc3339;

use super::error::DepotError;

pub const STATE_FORMAT: &str = "fenrir/depot-state/v1";
pub const BACKUP_FORMAT: &str = "fenrir/depot-backup/v1";
pub const BACKUP_FILE: &str = "backup.json";

/// One module as a distribution (or an override) put it on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleRecord {
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    /// The included distribution it came from, if not the product itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

/// A distribution installed on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledDistribution {
    /// `athenos-0.13.0`.
    pub id: String,
    /// SHA-256 of the canonical signed manifest.
    pub manifest_sha256: String,
    /// Signed channel at install time.
    pub channel: String,
    /// Release key that signed it.
    pub key_id: String,
    pub installed_at: String,
    /// The signed module set (pinned modules included at their
    /// distribution version; see `HostState::pins` for what runs).
    pub modules: BTreeMap<String, ModuleRecord>,
}

/// A single module installed with `install module` on top of the
/// distribution (a hotfix).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleOverride {
    pub version: Version,
    pub sha256: String,
    pub size: u64,
    /// The verified distribution whose signed manifest vouches for it.
    pub from: String,
    pub installed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostState {
    pub format: String,
    pub current: Option<InstalledDistribution>,
    pub previous: Option<InstalledDistribution>,
    /// Name of the backup (under `backups/`) that returns from `current` to
    /// what ran before it, if it is still there.
    #[serde(default)]
    pub backup: Option<String>,
    /// Modules `install distribution` keeps exactly as installed.
    #[serde(default)]
    pub pins: BTreeMap<String, Version>,
    #[serde(default)]
    pub overrides: BTreeMap<String, ModuleOverride>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            format: STATE_FORMAT.to_string(),
            current: None,
            previous: None,
            backup: None,
            pins: BTreeMap::new(),
            overrides: BTreeMap::new(),
        }
    }
}

/// One module in a backup: its archive and manifest from before the
/// change, or `None` when the change installed it fresh (undo = remove).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupModule {
    pub module: String,
    pub previous: Option<BackedUpModule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackedUpModule {
    pub version: Version,
    /// File name inside the backup directory.
    pub archive: String,
    /// SHA-256 of that archive (checked before a restore).
    pub archive_sha256: String,
    pub manifest: ModuleManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRecord {
    pub format: String,
    pub created_at: String,
    /// What ran before (`athenos-0.12.5`), `None` without a distribution.
    pub before: Option<String>,
    /// What the change installs (`athenos-0.13.0`, `module x 1.2.3`).
    pub after: String,
    pub modules: Vec<BackupModule>,
}

/// `<install_dir>/runtime/depot`: state file, blob cache, backups.
#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn blob_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    pub fn backup_dir(&self, name: &str) -> PathBuf {
        self.root.join("backups").join(name)
    }

    pub fn load(&self) -> Result<HostState, DepotError> {
        let path = self.state_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HostState::default())
            }
            Err(err) => return Err(DepotError::io(path.display(), err)),
        };
        let state: HostState = serde_json::from_slice(&bytes)
            .map_err(|err| DepotError::Io(format!("{} does not parse: {err}", path.display())))?;
        if state.format != STATE_FORMAT {
            return Err(DepotError::Io(format!(
                "{} has format {}, this host reads {STATE_FORMAT}",
                path.display(),
                state.format
            )));
        }
        Ok(state)
    }

    pub fn save(&self, state: &HostState) -> Result<(), DepotError> {
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|err| DepotError::Io(format!("state does not encode: {err}")))?;
        write_atomic(&self.state_path(), &bytes)
    }

    pub fn load_backup(&self, name: &str) -> Result<Option<BackupRecord>, DepotError> {
        let path = self.backup_dir(name).join(BACKUP_FILE);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|err| DepotError::Io(format!("{} does not parse: {err}", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(DepotError::io(path.display(), err)),
        }
    }

    pub fn save_backup(&self, name: &str, record: &BackupRecord) -> Result<(), DepotError> {
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|err| DepotError::Io(format!("backup record does not encode: {err}")))?;
        write_atomic(&self.backup_dir(name).join(BACKUP_FILE), &bytes)
    }

    /// Removes every backup except `keep`.
    pub fn prune_backups(&self, keep: Option<&str>) -> Result<(), DepotError> {
        let dir = self.root.join("backups");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(DepotError::io(dir.display(), err)),
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if Some(name.as_str()) == keep {
                continue;
            }
            fs::remove_dir_all(entry.path())
                .map_err(|err| DepotError::io(entry.path().display(), err))?;
        }
        Ok(())
    }
}

/// Writes `bytes` to `<path>.tmp`, syncs it and renames it over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), DepotError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| DepotError::io(parent.display(), err))?;
    }
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = fs::File::create(&tmp).map_err(|err| DepotError::io(tmp.display(), err))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|err| DepotError::io(tmp.display(), err))?;
    fs::rename(&tmp, path).map_err(|err| DepotError::io(path.display(), err))
}

pub fn now_rfc3339() -> String {
    system_time_to_rfc3339(SystemTime::now()).unwrap_or_else(|| "unknown".to_string())
}
