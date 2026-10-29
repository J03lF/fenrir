//! The install pipeline: prepare (select, verify, plan, download and check
//! every bundle; nothing on the host changes), then execute (lock, back up,
//! activate, migrate, start or roll, wait for health, record; on any
//! failure restore the backup).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;

use depot_protocol::rule::VersionPattern;
use depot_protocol::{Name, Sha256Digest};

use crate::domain::module::{
    ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum, ModuleManifest,
    ModuleSignatureDescriptor, SignatureAlgorithm,
};

use super::blob::{check_bytes, BlobCache};
use super::bundle;
use super::error::DepotError;
use super::plan::{plan_distribution, plan_module, plan_restore, Action, Plan, Restart};
use super::ports::{DepotSource, HostModule, ModuleHost};
use super::state::{
    now_rfc3339, BackedUpModule, BackupModule, BackupRecord, HostState, InstalledDistribution,
    ModuleOverride, ModuleRecord, StateStore, BACKUP_FORMAT,
};
use super::verify::{
    select_distribution, select_module, DistributionRequest, HostPolicy, ModuleCandidate,
    VerifiedDistribution,
};
use super::versions;

/// Progress lines for the operator.
pub type Progress<'a> = &'a (dyn Fn(String) + Send + Sync);

/// Everything the pipeline works with.
pub struct Depot<'a> {
    pub source: &'a dyn DepotSource,
    pub host: &'a dyn ModuleHost,
    pub store: &'a StateStore,
    pub policy: &'a HostPolicy,
}

#[derive(Debug, Clone)]
pub enum Intent {
    /// `install distribution` (also `rollback` without a local backup).
    Distribution {
        distribution: Box<VerifiedDistribution>,
        rollback: bool,
    },
    /// `install module`.
    Module { candidate: Box<ModuleCandidate> },
    /// `rollback` from the local backup of the last install.
    Restore {
        backup: String,
        record: BackupRecord,
    },
}

/// A bundle ready to activate: on disk, checked, checked again before use.
#[derive(Debug, Clone)]
pub struct StagedBundle {
    pub manifest: ModuleManifest,
    pub path: PathBuf,
    pub sha256: Sha256Digest,
    pub size: u64,
}

/// The result of `prepare_*`: shown as the plan, executed after the
/// operator confirmed.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub intent: Intent,
    pub plan: Plan,
    /// Newer matches passed over, with the reason.
    pub skipped: Vec<String>,
    /// Included host products (`fenrir-0.12.0`): checked, not installed.
    pub host_includes: Vec<String>,
    pub staged: BTreeMap<String, StagedBundle>,
    /// Bytes downloaded now (the rest was in the verified cache).
    pub downloaded: u64,
}

impl Prepared {
    /// Whether it can be executed (no problems; maybe nothing to change).
    pub fn is_installable(&self) -> bool {
        self.plan.problems.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub title: String,
    /// `module old → new` per change.
    pub changed: Vec<String>,
    pub migrations: Vec<String>,
    /// The backup that undoes this change, if one is kept.
    pub backup: Option<String>,
}

pub async fn prepare_distribution(
    depot: &Depot<'_>,
    request: &DistributionRequest,
    progress: Progress<'_>,
) -> Result<Prepared, DepotError> {
    let selection = select_distribution(depot.source, depot.policy, request).await?;
    let distribution = selection.distribution;
    progress(format!(
        "{}: signature ok (key {}), channel {} (signed), manifest sha256 {}",
        distribution.id(),
        distribution.root.key_id,
        distribution.root.channel,
        distribution.root.digest
    ));
    prepare_verified(depot, distribution, false, selection.skipped, progress).await
}

async fn prepare_verified(
    depot: &Depot<'_>,
    distribution: VerifiedDistribution,
    rollback: bool,
    skipped: Vec<String>,
    progress: Progress<'_>,
) -> Result<Prepared, DepotError> {
    let installed = depot.host.modules().await.map_err(DepotError::Io)?;
    let state = depot.store.load()?;
    let mut plan = plan_distribution(
        &distribution.id().to_string(),
        &distribution.target_modules(),
        &installed,
        &state,
    );
    let signer = distribution.root.key_id.to_string();
    let (staged, downloaded) = if plan.problems.is_empty() {
        stage(depot, &mut plan, &installed, &signer, progress).await?
    } else {
        (BTreeMap::new(), 0)
    };
    let host_includes = distribution
        .host_includes()
        .iter()
        .map(ToString::to_string)
        .collect();
    Ok(Prepared {
        intent: Intent::Distribution {
            distribution: Box::new(distribution),
            rollback,
        },
        plan,
        skipped,
        host_includes,
        staged,
        downloaded,
    })
}

pub async fn prepare_module(
    depot: &Depot<'_>,
    name: &Name,
    pattern: &VersionPattern,
    progress: Progress<'_>,
) -> Result<Prepared, DepotError> {
    let candidate = select_module(depot.source, depot.policy, name, pattern).await?;
    progress(format!(
        "{} {}: signed in {} (verified)",
        candidate.module.name, candidate.module.version, candidate.from
    ));
    let installed = depot.host.modules().await.map_err(DepotError::Io)?;
    let state = depot.store.load()?;
    let mut plan = plan_module(&candidate.module, &installed, &state);
    let (staged, downloaded) = if plan.problems.is_empty() {
        stage(depot, &mut plan, &installed, "depot", progress).await?
    } else {
        (BTreeMap::new(), 0)
    };
    Ok(Prepared {
        intent: Intent::Module {
            candidate: Box::new(candidate),
        },
        plan,
        skipped: Vec::new(),
        host_includes: Vec::new(),
        staged,
        downloaded,
    })
}

/// `rollback`: the local backup of the last install when it is complete,
/// otherwise the previous distribution, downloaded and verified again.
pub async fn prepare_rollback(
    depot: &Depot<'_>,
    progress: Progress<'_>,
) -> Result<Prepared, DepotError> {
    let state = depot.store.load()?;
    let Some(current) = &state.current else {
        return Err(DepotError::Refused(
            "nothing to roll back: no distribution is installed from the depot".to_string(),
        ));
    };
    if let Some(name) = &state.backup {
        if let Some(record) = depot.store.load_backup(name)? {
            if record.after == current.id {
                let staged = restore_bundles(depot.store, name, &record)?;
                let installed = depot.host.modules().await.map_err(DepotError::Io)?;
                let title = match &record.before {
                    Some(before) => format!("rollback {} → {before}", current.id),
                    None => format!("rollback {} → the modules from before it", current.id),
                };
                let plan = plan_restore(&title, &record, &installed);
                return Ok(Prepared {
                    intent: Intent::Restore {
                        backup: name.clone(),
                        record,
                    },
                    plan,
                    skipped: Vec::new(),
                    host_includes: Vec::new(),
                    staged,
                    downloaded: 0,
                });
            }
        }
    }
    let Some(previous) = &state.previous else {
        return Err(DepotError::Refused(format!(
            "nothing to roll back to: {} has no previous distribution and no local backup",
            current.id
        )));
    };
    progress(format!(
        "no local backup for {}; downloading {} again",
        current.id, previous.id
    ));
    let id = previous
        .id
        .parse()
        .map_err(|err| DepotError::Io(format!("state names `{}`: {err}", previous.id)))?;
    let selection =
        select_distribution(depot.source, depot.policy, &DistributionRequest::Exact(id)).await?;
    let mut prepared =
        prepare_verified(depot, selection.distribution, true, Vec::new(), progress).await?;
    prepared.plan.title = format!("rollback {} → {}", current.id, previous.id);
    Ok(prepared)
}

/// Downloads (or takes from the verified cache) every changed bundle and
/// checks it: SHA-256 and size against the signed manifest, archive
/// layout, the bundle's own manifest against the signed name and version.
async fn stage(
    depot: &Depot<'_>,
    plan: &mut Plan,
    installed: &[HostModule],
    signer: &str,
    progress: Progress<'_>,
) -> Result<(BTreeMap<String, StagedBundle>, u64), DepotError> {
    let cache = BlobCache::new(depot.store.blob_dir());
    let mut staged = BTreeMap::new();
    let mut downloaded = 0u64;
    for entry in plan
        .entries
        .iter_mut()
        .filter(|entry| entry.action.changes())
    {
        let Some(target) = &entry.source else {
            continue;
        };
        let fetched = cache
            .ensure(depot.source, &target.sha256, target.size)
            .await
            .map_err(|err| match err {
                DepotError::Bundle(message) => {
                    DepotError::Bundle(format!("{} {}: {message}", target.name, target.version))
                }
                other => other,
            })?;
        if fetched {
            downloaded = downloaded.saturating_add(target.size);
        }
        let bytes = cache.read(&target.sha256, target.size)?;
        let info = bundle::inspect(&bytes).map_err(|err| {
            DepotError::Bundle(format!("{} {}: {err}", target.name, target.version))
        })?;
        progress(format!(
            "  {} {} ({}) {}: sha256 ok",
            target.name,
            target.version,
            target.platform,
            if fetched { "downloaded" } else { "cached" }
        ));
        let manifest = module_manifest(target, info.manifest, signer)?;
        let known: HashSet<&String> = installed
            .iter()
            .find(|module| module.name == target.name)
            .map(|module| module.migrations.iter().collect())
            .unwrap_or_default();
        entry.migrations = info
            .migrations
            .into_iter()
            .filter(|file| !known.contains(file))
            .collect();
        staged.insert(
            target.name.clone(),
            StagedBundle {
                manifest,
                path: cache.path(&target.sha256),
                sha256: target.sha256,
                size: target.size,
            },
        );
    }
    Ok((staged, downloaded))
}

/// The manifest the module storage records: the bundle's own (for its
/// dependencies and runtime data) when it names the signed module and
/// version, with the artifact fields taken from the signed manifest.
fn module_manifest(
    target: &super::verify::TargetModule,
    own: Option<ModuleManifest>,
    signer: &str,
) -> Result<ModuleManifest, DepotError> {
    let mut manifest = match own {
        Some(manifest) => {
            let same_version = versions::from_manifest(&manifest.version)
                .is_ok_and(|version| version == target.version);
            if manifest.id != target.name || !same_version {
                return Err(DepotError::Bundle(format!(
                    "the bundle's own manifest says {} {}, the signed distribution {} {}",
                    manifest.id, manifest.version, target.name, target.version
                )));
            }
            manifest
        }
        None => ModuleManifest {
            id: target.name.clone(),
            version: versions::to_manifest(&target.version)?,
            title: None,
            description: None,
            fenrir_version: None,
            authors: Vec::new(),
            license: None,
            artifact: placeholder_artifact(),
            signature: placeholder_signature(),
            dependencies: Vec::new(),
            tags: Vec::new(),
            published_at: None,
        },
    };
    manifest.artifact = ModuleArtifactDescriptor {
        download_url: format!(
            "depot:sha256/{}/{}-{}.tar.gz",
            target.sha256, target.name, target.version
        ),
        checksum: ModuleChecksum {
            algorithm: ChecksumAlgorithm::Sha256,
            hash: target.sha256.to_hex(),
        },
        content_type: Some("application/gzip".to_string()),
        size_bytes: Some(target.size),
    };
    manifest.signature = ModuleSignatureDescriptor {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: format!("depot:{signer}"),
        signature: String::new(),
    };
    Ok(manifest)
}

fn placeholder_artifact() -> ModuleArtifactDescriptor {
    ModuleArtifactDescriptor {
        download_url: String::new(),
        checksum: ModuleChecksum {
            algorithm: ChecksumAlgorithm::Sha256,
            hash: String::new(),
        },
        content_type: None,
        size_bytes: None,
    }
}

fn placeholder_signature() -> ModuleSignatureDescriptor {
    ModuleSignatureDescriptor {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: String::new(),
        signature: String::new(),
    }
}

fn restore_bundles(
    store: &StateStore,
    name: &str,
    record: &BackupRecord,
) -> Result<BTreeMap<String, StagedBundle>, DepotError> {
    let dir = store.backup_dir(name);
    let mut staged = BTreeMap::new();
    for module in &record.modules {
        let Some(previous) = &module.previous else {
            continue;
        };
        let path = dir.join(&previous.archive);
        let sha256: Sha256Digest = previous.archive_sha256.parse().map_err(|_| {
            DepotError::Io(format!("backup {name}: bad digest for {}", module.module))
        })?;
        let size = fs::metadata(&path)
            .map_err(|err| DepotError::io(path.display(), err))?
            .len();
        staged.insert(
            module.module.clone(),
            StagedBundle {
                manifest: previous.manifest.clone(),
                path,
                sha256,
                size,
            },
        );
    }
    Ok(staged)
}

/// Reads a staged bundle and checks it once more right before use.
fn load_bundle(staged: &StagedBundle) -> Result<ModuleBundle, DepotError> {
    let bytes = fs::read(&staged.path).map_err(|err| DepotError::io(staged.path.display(), err))?;
    check_bytes(&bytes, &staged.sha256, staged.size)?;
    Ok(ModuleBundle {
        manifest: staged.manifest.clone(),
        archive: bytes,
        signature: Vec::new(),
        checksum: staged.sha256.as_bytes().to_vec(),
    })
}

/// Only one depot operation at a time (`<depot>/lock`).
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn take(store: &StateStore) -> Result<Self, DepotError> {
        let path = store.root().join("lock");
        fs::create_dir_all(store.root())
            .map_err(|err| DepotError::io(store.root().display(), err))?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(Self { path }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(DepotError::Refused(format!(
                    "another depot operation is running (lock {}); if none is, remove the file",
                    path.display()
                )))
            }
            Err(err) => Err(DepotError::io(path.display(), err)),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Changes the host state under the depot lock (`pin`, `unpin`, ...).
pub fn update_state<T>(
    store: &StateStore,
    change: impl FnOnce(&mut HostState) -> Result<T, DepotError>,
) -> Result<T, DepotError> {
    let _lock = Lock::take(store)?;
    let mut state = store.load()?;
    let result = change(&mut state)?;
    store.save(&state)?;
    Ok(result)
}

fn backup_name(title: &str) -> String {
    let stamp: String = now_rfc3339()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let label: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{stamp}-{label}")
}

/// Executes a prepared plan; see the module docs.
pub async fn execute(
    depot: &Depot<'_>,
    prepared: &Prepared,
    progress: Progress<'_>,
) -> Result<Outcome, DepotError> {
    if !prepared.is_installable() {
        return Err(DepotError::Refused(prepared.plan.problems.join("\n")));
    }
    let _lock = Lock::take(depot.store)?;
    let mut state = depot.store.load()?;
    let plan = &prepared.plan;
    let changes: Vec<_> = plan.changes().collect();
    let mut kept_backup = None;
    let mut migrations = Vec::new();

    if !changes.is_empty() {
        // Every bundle once more, before anything changes.
        for staged in prepared.staged.values() {
            load_bundle(staged)?;
        }
        let name = backup_name(&plan.title);
        let dir = depot.store.backup_dir(&name);
        fs::create_dir_all(&dir).map_err(|err| DepotError::io(dir.display(), err))?;
        let mut record = BackupRecord {
            format: BACKUP_FORMAT.to_string(),
            created_at: now_rfc3339(),
            before: state.current.as_ref().map(|current| current.id.clone()),
            after: match &prepared.intent {
                Intent::Distribution { distribution, .. } => distribution.id().to_string(),
                _ => plan.title.clone(),
            },
            modules: Vec::new(),
        };
        progress(format!(
            "backing up {} module(s) to {}",
            changes.len(),
            dir.display()
        ));
        for entry in &changes {
            let archive = format!("{}.tar.gz", entry.module);
            let path = dir.join(&archive);
            let previous = match depot
                .host
                .backup(&entry.module, &path)
                .await
                .map_err(|err| DepotError::Io(format!("backup of {}: {err}", entry.module)))?
            {
                Some(manifest) => {
                    let bytes =
                        fs::read(&path).map_err(|err| DepotError::io(path.display(), err))?;
                    Some(BackedUpModule {
                        version: versions::installed(&entry.module, &manifest.version, false)
                            .map_err(DepotError::Refused)?,
                        archive,
                        archive_sha256: Sha256Digest::of(&bytes).to_hex(),
                        manifest,
                    })
                }
                None => None,
            };
            record.modules.push(BackupModule {
                module: entry.module.clone(),
                previous,
            });
        }
        depot.store.save_backup(&name, &record)?;

        match apply(depot, prepared, progress).await {
            Ok(applied) => migrations = applied,
            Err(message) => {
                progress(format!("{message}; rolling back"));
                let rolled_back = match restore(depot, &name, &record, progress).await {
                    Ok(()) => true,
                    Err(err) => {
                        progress(format!("rollback failed: {err}"));
                        false
                    }
                };
                return Err(DepotError::Install {
                    message,
                    rolled_back,
                });
            }
        }
        kept_backup = Some(name);
    }

    let changed: Vec<String> = changes
        .iter()
        .map(|entry| {
            format!(
                "{} {} → {}",
                entry.module,
                entry
                    .installed
                    .as_ref()
                    .map_or_else(|| "-".to_string(), ToString::to_string),
                entry
                    .target
                    .as_ref()
                    .map_or_else(|| "removed".to_string(), ToString::to_string)
            )
        })
        .collect();
    let backup = record_state(&mut state, prepared, kept_backup);
    depot.store.save(&state)?;
    depot.store.prune_backups(state.backup.as_deref())?;
    prune_cache(depot.store, &state);
    Ok(Outcome {
        title: plan.title.clone(),
        changed,
        migrations,
        backup,
    })
}

/// Stops, activates, migrates, starts or rolls, and waits for health.
async fn apply(
    depot: &Depot<'_>,
    prepared: &Prepared,
    progress: Progress<'_>,
) -> Result<Vec<String>, String> {
    let host = depot.host;
    let changes: Vec<_> = prepared.plan.changes().collect();
    for entry in &changes {
        if matches!(entry.restart, Restart::Restart | Restart::Stop) {
            host.stop(&entry.module).await?;
        }
    }
    let mut started = Vec::new();
    for entry in &changes {
        if entry.action == Action::Remove {
            host.remove(&entry.module).await?;
            progress(format!("  {} removed", entry.module));
            continue;
        }
        let staged = prepared
            .staged
            .get(&entry.module)
            .ok_or_else(|| format!("no verified bundle staged for {}", entry.module))?;
        let bundle = load_bundle(staged).map_err(|err| err.to_string())?;
        host.activate(bundle).await?;
        progress(format!(
            "  {} {} activated",
            entry.module,
            entry
                .target
                .as_ref()
                .map_or_else(String::new, ToString::to_string)
        ));
        started.push(entry.module.clone());
    }
    let migrating: Vec<String> = changes
        .iter()
        .filter(|entry| !entry.migrations.is_empty())
        .map(|entry| entry.module.clone())
        .collect();
    let applied = if migrating.is_empty() {
        Vec::new()
    } else {
        let applied = host.apply_migrations(&migrating).await?;
        progress(format!("  migrations applied: {}", applied.join(", ")));
        applied
    };
    host.start(&started).await?;
    for entry in &changes {
        if entry.restart == Restart::Rolling {
            host.rolling_restart(&entry.module).await?;
            progress(format!("  {} rolled", entry.module));
        }
    }
    for module in &started {
        host.wait_healthy(module).await?;
        progress(format!("  {module} healthy"));
    }
    Ok(applied)
}

/// Puts back what a backup holds; used by the automatic rollback.
async fn restore(
    depot: &Depot<'_>,
    name: &str,
    record: &BackupRecord,
    progress: Progress<'_>,
) -> Result<(), String> {
    let host = depot.host;
    let staged = restore_bundles(depot.store, name, record).map_err(|err| err.to_string())?;
    for module in &record.modules {
        let _ = host.stop(&module.module).await;
    }
    let mut started = Vec::new();
    for module in &record.modules {
        match staged.get(&module.module) {
            Some(bundle) => {
                host.activate(load_bundle(bundle).map_err(|err| err.to_string())?)
                    .await?;
                started.push(module.module.clone());
            }
            None => host.remove(&module.module).await?,
        }
    }
    host.start(&started).await?;
    for module in &started {
        host.wait_healthy(module).await?;
    }
    progress(format!(
        "restored {} module(s) from backup {name}; database migrations are not reverted",
        record.modules.len()
    ));
    Ok(())
}

/// Updates the host state after a successful execution; returns the
/// backup that is kept.
fn record_state(
    state: &mut HostState,
    prepared: &Prepared,
    backup: Option<String>,
) -> Option<String> {
    let runs_distribution: Vec<String> = prepared
        .plan
        .entries
        .iter()
        .filter(|entry| entry.action.changes() || entry.action == Action::Unchanged)
        .map(|entry| entry.module.clone())
        .collect();
    match &prepared.intent {
        Intent::Distribution { distribution, .. } => {
            let id = distribution.id().to_string();
            let modules = prepared
                .plan
                .entries
                .iter()
                .filter(|entry| entry.action != Action::OptionalSkipped)
                .filter_map(|entry| entry.source.as_ref())
                .map(|target| {
                    (
                        target.name.clone(),
                        ModuleRecord {
                            version: target.version.clone(),
                            sha256: target.sha256.to_hex(),
                            size: target.size,
                            via: target.via.clone(),
                        },
                    )
                })
                .collect();
            let record = InstalledDistribution {
                id: id.clone(),
                manifest_sha256: distribution.root.digest.to_hex(),
                channel: distribution.root.channel.to_string(),
                key_id: distribution.root.key_id.to_string(),
                installed_at: now_rfc3339(),
                modules,
            };
            let same = state
                .current
                .as_ref()
                .is_some_and(|current| current.id == id);
            if !same {
                state.previous = state.current.take();
            }
            state.current = Some(record);
            if backup.is_some() || !same {
                state.backup = backup;
            }
            for module in runs_distribution {
                state.overrides.remove(&module);
            }
        }
        Intent::Module { candidate } => {
            let module = &candidate.module;
            state.overrides.insert(
                module.name.clone(),
                ModuleOverride {
                    version: module.version.clone(),
                    sha256: module.sha256.to_hex(),
                    size: module.size,
                    from: candidate.from.to_string(),
                    installed_at: now_rfc3339(),
                },
            );
            // The hotfix backup is only for the automatic rollback.
            return state.backup.clone();
        }
        Intent::Restore { record, .. } => {
            state.current = state.previous.take();
            state.backup = None;
            for module in &record.modules {
                state.overrides.remove(&module.module);
            }
            return None;
        }
    }
    state.backup.clone()
}

/// Keeps the bundles of the current and previous distribution and of the
/// overrides; everything else in the cache goes.
fn prune_cache(store: &StateStore, state: &HostState) {
    let mut keep: HashSet<String> = HashSet::new();
    for distribution in [&state.current, &state.previous].into_iter().flatten() {
        keep.extend(
            distribution
                .modules
                .values()
                .map(|record| record.sha256.clone()),
        );
    }
    keep.extend(state.overrides.values().map(|over| over.sha256.clone()));
    if let Err(err) = BlobCache::new(store.blob_dir()).prune(&keep) {
        tracing::warn!(error = %err, "depot blob cache prune failed");
    }
}
