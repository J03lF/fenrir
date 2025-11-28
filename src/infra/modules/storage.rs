use std::io::Cursor;
use std::path::PathBuf;
use std::time::SystemTime;

use async_trait::async_trait;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use tar::Archive;
use tokio::{fs, task};

use crate::config::ModuleStorageSection;
use crate::domain::module::{
    InstalledModule, ModuleBundle, ModuleId, ModuleInstallResult, ModuleInstallSource,
    ModuleInstallStatus, ModuleManifest, ModuleStorageError, ModuleStoragePort,
};
use crate::utils::messages;
use tracing::warn;

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

    fn metadata_dir(&self, id: &ModuleId) -> PathBuf {
        self.module_dir(id).join(".fenrir-meta")
    }

    fn install_metadata_path(&self, id: &ModuleId) -> PathBuf {
        self.metadata_dir(id).join("install.json")
    }

    fn manifest_path(&self, id: &ModuleId) -> PathBuf {
        self.metadata_dir(id).join("manifest.json")
    }

    fn archive_path(&self, id: &ModuleId, manifest: &ModuleManifest) -> PathBuf {
        let file_name = artifact_file_name(manifest);
        self.metadata_dir(id).join(file_name)
    }

    fn signature_path(&self, id: &ModuleId) -> PathBuf {
        self.metadata_dir(id).join("signature.bin")
    }

    fn checksum_path(&self, id: &ModuleId) -> PathBuf {
        self.metadata_dir(id).join("checksum.bin")
    }

    fn download_url_path(&self, id: &ModuleId) -> PathBuf {
        self.metadata_dir(id).join("download")
    }

    async fn read_installed(
        &self,
        id: &ModuleId,
    ) -> Result<Option<InstalledModule>, ModuleStorageError> {
        let manifest_path = self.manifest_path(id);
        match fs::read(&manifest_path).await {
            Ok(bytes) => {
                let manifest: ModuleManifest = serde_json::from_slice(&bytes).map_err(|err| {
                    ModuleStorageError::InvalidState(
                        messages::infra::modules::storage::manifest_parse_failed(
                            manifest_path.display(),
                            err,
                        ),
                    )
                })?;
                let installed_at = fs::metadata(&manifest_path)
                    .await
                    .and_then(|meta| meta.modified())
                    .unwrap_or_else(|_| SystemTime::now());
                Ok(Some(InstalledModule {
                    manifest,
                    installed_at,
                    path: self.module_dir(id).to_string_lossy().to_string(),
                    source: self.read_install_source(id).await,
                }))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ModuleStorageError::Io(
                messages::infra::modules::storage::manifest_read_failed(
                    manifest_path.display(),
                    err,
                ),
            )),
        }
    }

    async fn read_install_source(&self, id: &ModuleId) -> ModuleInstallSource {
        let metadata_path = self.install_metadata_path(id);
        match fs::read(&metadata_path).await {
            Ok(bytes) => serde_json::from_slice::<ModuleInstallMetadata>(&bytes)
                .map(|meta| meta.source)
                .unwrap_or_else(|err| {
                    warn!(
                        target = "modules::storage",
                        module = %id,
                        path = %metadata_path.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::storage::INSTALL_METADATA_PARSE_FAILED
                    );
                    ModuleInstallSource::Distribution
                }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                ModuleInstallSource::Distribution
            }
            Err(err) => {
                warn!(
                    target = "modules::storage",
                    module = %id,
                    path = %metadata_path.display(),
                    error = %err,
                    "{}",
                    messages::infra::modules::storage::INSTALL_METADATA_READ_FAILED
                );
                ModuleInstallSource::Distribution
            }
        }
    }

    async fn persist_install_metadata(
        &self,
        id: &ModuleId,
        source: ModuleInstallSource,
    ) -> Result<(), ModuleStorageError> {
        let metadata = ModuleInstallMetadata { source };
        let metadata_bytes = serde_json::to_vec_pretty(&metadata).map_err(|err| {
            ModuleStorageError::InvalidState(
                messages::infra::modules::storage::install_metadata_encode_failed(id, err),
            )
        })?;
        let metadata_path = self.install_metadata_path(id);
        fs::write(&metadata_path, &metadata_bytes)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(
                    messages::infra::modules::storage::install_metadata_write_failed(
                        metadata_path.display(),
                        err,
                    ),
                )
            })
    }
}

#[async_trait]
impl ModuleStoragePort for FilesystemModuleStorage {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
        let mut entries = Vec::new();
        let mut dir = fs::read_dir(&self.install_dir).await.map_err(|err| {
            ModuleStorageError::Io(messages::infra::modules::storage::open_dir_failed(
                self.install_dir.display(),
                err,
            ))
        })?;
        while let Some(entry) = dir.next_entry().await.map_err(|err| {
            ModuleStorageError::Io(messages::infra::modules::storage::iterate_dir_failed(
                self.install_dir.display(),
                err,
            ))
        })? {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map_err(|err| {
                    ModuleStorageError::Io(
                        messages::infra::modules::storage::file_type_read_failed(
                            path.display(),
                            err,
                        ),
                    )
                })?
                .is_dir()
            {
                continue;
            }
            let Some(module_name) = path.file_name().and_then(|name| name.to_str()) else {
                warn!(
                    path = %path.display(),
                    "{}",
                    messages::infra::modules::storage::SKIP_NON_UTF8_ENTRY
                );
                continue;
            };

            if module_name.starts_with('.') {
                warn!(
                    path = %path.display(),
                    name = module_name,
                    "{}",
                    messages::infra::modules::storage::SKIP_HIDDEN_ENTRY
                );
                continue;
            }

            let module_id = match ModuleId::new(module_name) {
                Ok(id) => id,
                Err(err) => {
                    warn!(
                        path = %path.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::storage::SKIP_INVALID_IDENTIFIER
                    );
                    continue;
                }
            };
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
        source: ModuleInstallSource,
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
                return Err(ModuleStorageError::InvalidState(
                    messages::infra::modules::storage::installed_version_newer(
                        installed_version,
                        version,
                    ),
                ));
            }
        }
        let module_dir = self.module_dir(&id);

        if module_dir.exists() {
            fs::remove_dir_all(&module_dir).await.map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::clean_dir_failed(
                    module_dir.display(),
                    err,
                ))
            })?;
        }

        fs::create_dir_all(&module_dir).await.map_err(|err| {
            ModuleStorageError::Io(messages::infra::modules::storage::prepare_dir_failed(
                module_dir.display(),
                err,
            ))
        })?;

        extract_module_archive(&module_dir, &bundle.archive).await?;

        let metadata_dir = self.metadata_dir(&id);
        fs::create_dir_all(&metadata_dir).await.map_err(|err| {
            ModuleStorageError::Io(
                messages::infra::modules::storage::prepare_metadata_dir_failed(
                    metadata_dir.display(),
                    err,
                ),
            )
        })?;

        let manifest_bytes = serde_json::to_vec_pretty(&bundle.manifest).map_err(|err| {
            ModuleStorageError::InvalidState(
                messages::infra::modules::storage::manifest_encode_failed(&bundle.manifest.id, err),
            )
        })?;
        let manifest_path = self.manifest_path(&id);
        fs::write(&manifest_path, &manifest_bytes)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::manifest_write_failed(
                    manifest_path.display(),
                    err,
                ))
            })?;

        let artifact_path = self.archive_path(&id, &bundle.manifest);
        fs::write(&artifact_path, &bundle.archive)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::artifact_write_failed(
                    artifact_path.display(),
                    err,
                ))
            })?;

        fs::write(self.signature_path(&id), &bundle.signature)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::signature_write_failed(
                    &id, err,
                ))
            })?;
        fs::write(self.checksum_path(&id), &bundle.checksum)
            .await
            .map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::checksum_write_failed(
                    &id, err,
                ))
            })?;

        // Store the original download URL for reference
        fs::write(
            self.download_url_path(&id),
            bundle.manifest.artifact.download_url.as_bytes(),
        )
        .await
        .map_err(|err| {
            ModuleStorageError::Io(
                messages::infra::modules::storage::download_url_write_failed(&id, err),
            )
        })?;

        self.persist_install_metadata(&id, source).await?;

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
            source,
        })
    }

    async fn remove(&self, id: &ModuleId) -> Result<(), ModuleStorageError> {
        let module_dir = self.module_dir(id);
        match fs::remove_dir_all(&module_dir).await {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ModuleStorageError::Io(
                messages::infra::modules::storage::remove_dir_failed(module_dir.display(), err),
            )),
        }
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
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .unwrap_or_else(|| format!("{}-{}.module", manifest.id, manifest.version))
}

async fn extract_module_archive(
    destination: &std::path::Path,
    archive_bytes: &[u8],
) -> Result<(), ModuleStorageError> {
    let bytes = archive_bytes.to_vec();
    let destination = destination.to_path_buf();

    task::spawn_blocking(move || -> Result<(), ModuleStorageError> {
        let cursor = Cursor::new(bytes);
        let decoder = GzDecoder::new(cursor);
        let mut archive = Archive::new(decoder);
        archive.unpack(&destination).map_err(|err| {
            ModuleStorageError::Io(messages::infra::modules::storage::unpack_failed(
                destination.display(),
                err,
            ))
        })?;

        flatten_module_root(&destination)?;
        Ok(())
    })
    .await
    .map_err(|err| {
        ModuleStorageError::Io(messages::infra::modules::storage::extraction_task_failed(
            err,
        ))
    })??;

    Ok(())
}

fn flatten_module_root(destination: &std::path::Path) -> Result<(), ModuleStorageError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(destination).map_err(|err| {
        ModuleStorageError::Io(messages::infra::modules::storage::list_extracted_failed(
            destination.display(),
            err,
        ))
    })? {
        let entry = entry.map_err(|err| {
            ModuleStorageError::Io(
                messages::infra::modules::storage::access_extracted_entry_failed(
                    destination.display(),
                    err,
                ),
            )
        })?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == ".DS_Store" || name_str.starts_with("._") {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        entries.push(entry);
    }

    if entries.len() == 1 {
        let entry = entries.pop().unwrap();
        let file_type = entry.file_type().map_err(|err| {
            ModuleStorageError::Io(messages::infra::modules::storage::inspect_entry_failed(
                entry.path().display(),
                err,
            ))
        })?;

        if file_type.is_dir() {
            let inner = entry.path();
            for child in std::fs::read_dir(&inner).map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::read_nested_dir_failed(
                    inner.display(),
                    err,
                ))
            })? {
                let child = child.map_err(|err| {
                    ModuleStorageError::Io(
                        messages::infra::modules::storage::access_nested_entry_failed(
                            inner.display(),
                            err,
                        ),
                    )
                })?;
                let target = destination.join(child.file_name());
                std::fs::rename(child.path(), &target).map_err(|err| {
                    ModuleStorageError::Io(
                        messages::infra::modules::storage::relocate_entry_failed(
                            child.path().display(),
                            target.display(),
                            err,
                        ),
                    )
                })?;
            }

            std::fs::remove_dir_all(&inner).map_err(|err| {
                ModuleStorageError::Io(messages::infra::modules::storage::remove_nested_dir_failed(
                    inner.display(),
                    err,
                ))
            })?;
        }
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct ModuleInstallMetadata {
    source: ModuleInstallSource,
}
