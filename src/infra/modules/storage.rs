use std::path::PathBuf;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::config::ModuleStorageSection;
use crate::domain::module::{
    InstalledModule, ModuleBundle, ModuleId, ModuleInstallResult, ModuleInstallStatus,
    ModuleManifest, ModuleStorageError, ModuleStoragePort,
};

#[derive(Debug, Clone)]
pub struct FilesystemModuleStorage {
    install_dir: PathBuf,
    _cache_dir: Option<PathBuf>,
}

impl FilesystemModuleStorage {
    pub fn new(config: &ModuleStorageSection) -> Result<Self, ModuleStorageInitError> {
        let install_dir = resolve_path(&config.install_dir)?;
        if !install_dir.exists() {
            std::fs::create_dir_all(&install_dir).map_err(|err| ModuleStorageInitError::Io {
                path: install_dir.clone(),
                source: err,
            })?;
        }
        let cache_dir = match &config.cache_dir {
            Some(path) if !path.trim().is_empty() => {
                let resolved = resolve_path(path)?;
                if !resolved.exists() {
                    std::fs::create_dir_all(&resolved).map_err(|err| {
                        ModuleStorageInitError::Io {
                            path: resolved.clone(),
                            source: err,
                        }
                    })?;
                }
                Some(resolved)
            }
            _ => None,
        };
        Ok(Self {
            install_dir,
            _cache_dir: cache_dir,
        })
    }

    fn module_dir(&self, id: &ModuleId) -> PathBuf {
        self.install_dir.join(id.as_str())
    }

    fn manifest_path(&self, id: &ModuleId) -> PathBuf {
        self.module_dir(id).join("manifest.json")
    }

    fn archive_path(&self, id: &ModuleId, manifest: &ModuleManifest) -> PathBuf {
        let file_name = artifact_file_name(manifest);
        self.module_dir(id).join(file_name)
    }

    fn signature_path(&self, id: &ModuleId) -> PathBuf {
        self.module_dir(id).join("signature.bin")
    }

    fn checksum_path(&self, id: &ModuleId) -> PathBuf {
        self.module_dir(id).join("checksum.bin")
    }

    async fn read_installed(
        &self,
        id: &ModuleId,
    ) -> Result<Option<InstalledModule>, ModuleStorageError> {
        let manifest_path = self.manifest_path(id);
        match fs::read(&manifest_path).await {
            Ok(bytes) => {
                let manifest: ModuleManifest = serde_json::from_slice(&bytes).map_err(|err| {
                    ModuleStorageError::InvalidState(format!(
                        "failed to parse manifest {}: {err}",
                        manifest_path.display()
                    ))
                })?;
                let installed_at = fs::metadata(&manifest_path)
                    .await
                    .and_then(|meta| meta.modified())
                    .unwrap_or_else(|_| SystemTime::now());
                Ok(Some(InstalledModule {
                    manifest,
                    installed_at,
                    path: self.module_dir(id).to_string_lossy().to_string(),
                }))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ModuleStorageError::Io(format!(
                "cannot read manifest {}: {err}",
                manifest_path.display()
            ))),
        }
    }
}

#[async_trait]
impl ModuleStoragePort for FilesystemModuleStorage {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
        let mut entries = Vec::new();
        let mut dir = fs::read_dir(&self.install_dir).await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "cannot open module directory {}: {err}",
                self.install_dir.display()
            ))
        })?;
        while let Some(entry) = dir.next_entry().await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "failed to iterate module directory {}: {err}",
                self.install_dir.display()
            ))
        })? {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map_err(|err| {
                    ModuleStorageError::Io(format!(
                        "cannot read file type {}: {err}",
                        path.display()
                    ))
                })?
                .is_dir()
            {
                continue;
            }
            let module_id = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    ModuleStorageError::InvalidState(format!(
                        "module directory {} not valid UTF-8",
                        path.display()
                    ))
                })?;
            let module_id = ModuleId::new(module_id).map_err(|err| {
                ModuleStorageError::InvalidState(format!(
                    "invalid module id in storage {}: {err}",
                    path.display()
                ))
            })?;
            if let Some(installed) = self.read_installed(&module_id).await? {
                entries.push(installed);
            }
        }
        entries.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(entries)
    }

    async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError> {
        self.read_installed(id).await
    }

    async fn stage_and_activate(
        &self,
        bundle: ModuleBundle,
    ) -> Result<ModuleInstallResult, ModuleStorageError> {
        let id = bundle
            .manifest
            .module_id()
            .map_err(|err| ModuleStorageError::InvalidState(err.to_string()))?;
        let version = bundle.manifest.module_version();
        let current = self.read_installed(&id).await?;
        if let Some(installed) = &current {
            let installed_version = installed.manifest.module_version();
            if installed_version.as_semver() > version.as_semver() {
                return Err(ModuleStorageError::InvalidState(format!(
                    "installed version {} newer than requested {}",
                    installed_version, version
                )));
            }
        }
        let module_dir = self.module_dir(&id);
        fs::create_dir_all(&module_dir).await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "cannot prepare module directory {}: {err}",
                module_dir.display()
            ))
        })?;

        let manifest_bytes = serde_json::to_vec_pretty(&bundle.manifest).map_err(|err| {
            ModuleStorageError::InvalidState(format!(
                "failed to encode manifest for {}: {err}",
                bundle.manifest.id
            ))
        })?;
        let manifest_path = self.manifest_path(&id);
        fs::write(&manifest_path, &manifest_bytes)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(format!(
                    "cannot write manifest {}: {err}",
                    manifest_path.display()
                ))
            })?;

        let artifact_path = self.archive_path(&id, &bundle.manifest);
        let mut file = fs::File::create(&artifact_path).await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "cannot create artifact {}: {err}",
                artifact_path.display()
            ))
        })?;
        file.write_all(&bundle.archive).await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "cannot write artifact {}: {err}",
                artifact_path.display()
            ))
        })?;
        file.flush().await.map_err(|err| {
            ModuleStorageError::Io(format!(
                "cannot flush artifact {}: {err}",
                artifact_path.display()
            ))
        })?;

        fs::write(self.signature_path(&id), &bundle.signature)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(format!("cannot write signature for {}: {err}", id))
            })?;
        fs::write(self.checksum_path(&id), &bundle.checksum)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(format!("cannot write checksum for {}: {err}", id))
            })?;

        let status = match current {
            None => ModuleInstallStatus::Installed,
            Some(installed) => {
                let installed_version = installed.manifest.module_version();
                if installed_version.as_semver() == version.as_semver() {
                    ModuleInstallStatus::AlreadyCurrent
                } else {
                    ModuleInstallStatus::Updated
                }
            }
        };

        Ok(ModuleInstallResult {
            status,
            manifest: bundle.manifest,
            path: module_dir.to_string_lossy().to_string(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModuleStorageInitError {
    #[error("module storage path {path} inaccessible: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn resolve_path(path: &str) -> Result<PathBuf, ModuleStorageInitError> {
    Ok(PathBuf::from(path))
}

fn artifact_file_name(manifest: &ModuleManifest) -> String {
    let raw = manifest.artifact.download_url.trim();
    raw.split('/')
        .last()
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| format!("{}-{}.module", manifest.id, manifest.version))
}
