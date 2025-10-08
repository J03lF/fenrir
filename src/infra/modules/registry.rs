use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use semver::Version;
use serde::Deserialize;
use tokio::fs;

use crate::config::ModuleRegistrySection;
use crate::domain::module::{
    ModuleBundle, ModuleId, ModuleManifest, ModuleRegistryError, ModuleRegistryPort,
    ModuleSearchQuery, ModuleSummary, ModuleVersion,
};

#[derive(Debug, Clone)]
pub struct FilesystemModuleRegistry {
    root: PathBuf,
    index_file: String,
    allow_offline: bool,
}

impl FilesystemModuleRegistry {
    pub fn new(config: &ModuleRegistrySection) -> Result<Self, ModuleRegistryInitError> {
        let root = resolve_endpoint(&config.endpoint)?;
        let canonical_root = if root.exists() {
            std::fs::canonicalize(&root).map_err(|err| ModuleRegistryInitError::Io {
                path: root.clone(),
                source: err,
            })?
        } else {
            root
        };
        Ok(Self {
            root: canonical_root,
            index_file: config.index_file.clone(),
            allow_offline: config.allow_offline,
        })
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(&self.index_file)
    }

    async fn load_index(&self) -> Result<RegistryIndex, ModuleRegistryError> {
        let path = self.index_path();
        match fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|err| {
                ModuleRegistryError::Protocol(format!(
                    "index {} invalid JSON: {err}",
                    path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && self.allow_offline => {
                Ok(RegistryIndex {
                    modules: Vec::new(),
                })
            }
            Err(err) => Err(ModuleRegistryError::Unavailable(format!(
                "index {} not accessible: {err}",
                path.display()
            ))),
        }
    }

    fn manifest_path(&self, module: &ModuleId, version: &ModuleVersion) -> PathBuf {
        self.root
            .join(module.as_str())
            .join(version.as_semver().to_string())
            .join("manifest.json")
    }

    async fn read_manifest(&self, path: &Path) -> Result<ModuleManifest, ModuleRegistryError> {
        let bytes = fs::read(path).await.map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => ModuleRegistryError::NotFound {
                module: path.display().to_string(),
            },
            _ => ModuleRegistryError::Unavailable(format!(
                "manifest {} inaccessible: {err}",
                path.display()
            )),
        })?;
        serde_json::from_slice(&bytes).map_err(|err| {
            ModuleRegistryError::Protocol(format!(
                "manifest {} invalid JSON: {err}",
                path.display()
            ))
        })
    }

    fn resolve_artifact_path(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<PathBuf, ModuleRegistryError> {
        let raw = manifest.artifact.download_url.trim();
        if raw.is_empty() {
            return Err(ModuleRegistryError::Protocol(
                "artifact download_url must not be empty".to_string(),
            ));
        }
        if let Some(path) = raw.strip_prefix("file://") {
            Ok(PathBuf::from(path))
        } else if raw.starts_with("http://") || raw.starts_with("https://") {
            Err(ModuleRegistryError::Protocol(
                "http(s) registry endpoints are not implemented".to_string(),
            ))
        } else {
            Ok(self.root.join(raw))
        }
    }
}

#[async_trait]
impl ModuleRegistryPort for FilesystemModuleRegistry {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError> {
        let index = self.load_index().await?;
        let mut summaries = Vec::new();
        let pattern = query.pattern.as_ref().map(|p| p.to_ascii_lowercase());
        for entry in index.modules {
            let module_id = ModuleId::new(&entry.id).map_err(|err| {
                ModuleRegistryError::Protocol(format!(
                    "invalid module id `{}` in index: {err}",
                    entry.id
                ))
            })?;
            let version = ModuleVersion::parse(&entry.version).map_err(|err| {
                ModuleRegistryError::Protocol(format!(
                    "invalid version `{}` for module {}: {err}",
                    entry.version, entry.id
                ))
            })?;
            if let Some(pattern) = pattern.as_ref() {
                if !entry.id.to_ascii_lowercase().contains(pattern)
                    && !entry
                        .title
                        .as_ref()
                        .map(|title| title.to_ascii_lowercase().contains(pattern))
                        .unwrap_or(false)
                {
                    continue;
                }
            }
            summaries.push(ModuleSummary {
                id: module_id,
                version,
                title: entry.title,
                description: entry.description,
                tags: entry.tags.unwrap_or_default(),
            });
        }
        summaries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(summaries)
    }

    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError> {
        let chosen_version = match version {
            Some(version) => version.clone(),
            None => {
                let index = self.load_index().await?;
                let mut versions: BTreeMap<Version, String> = BTreeMap::new();
                for entry in index.modules {
                    if entry.id == id.as_str() {
                        let version = ModuleVersion::parse(&entry.version).map_err(|err| {
                            ModuleRegistryError::Protocol(format!(
                                "invalid version `{}` for module {}: {err}",
                                entry.version, entry.id
                            ))
                        })?;
                        versions.insert(version.as_semver().clone(), entry.version);
                    }
                }
                let latest = versions
                    .iter()
                    .last()
                    .map(|(version, raw)| (version.clone(), raw.clone()))
                    .ok_or_else(|| ModuleRegistryError::NotFound {
                        module: id.to_string(),
                    })?;
                ModuleVersion(latest.0)
            }
        };
        let path = self.manifest_path(id, &chosen_version);
        self.read_manifest(&path).await
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        let artifact_path = self.resolve_artifact_path(manifest)?;
        let archive = fs::read(&artifact_path).await.map_err(|err| {
            ModuleRegistryError::Unavailable(format!(
                "artifact {} not accessible: {err}",
                artifact_path.display()
            ))
        })?;
        let signature_bytes =
            BASE64
                .decode(manifest.signature.signature.trim())
                .map_err(|err| {
                    ModuleRegistryError::Protocol(format!(
                        "signature for module {} invalid base64: {err}",
                        manifest.id
                    ))
                })?;
        let checksum_bytes =
            hex::decode(manifest.artifact.checksum.hash.trim()).map_err(|err| {
                ModuleRegistryError::Protocol(format!(
                    "checksum for module {} invalid hex: {err}",
                    manifest.id
                ))
            })?;
        Ok(ModuleBundle {
            manifest: manifest.clone(),
            archive,
            signature: signature_bytes,
            checksum: checksum_bytes,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModuleRegistryInitError {
    #[error("module registry path {path} inaccessible: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("module registry endpoint {0} is not supported")]
    UnsupportedEndpoint(String),
}

fn resolve_endpoint(endpoint: &str) -> Result<PathBuf, ModuleRegistryInitError> {
    if let Some(path) = endpoint.strip_prefix("file://") {
        Ok(PathBuf::from(path))
    } else if endpoint.contains("://") {
        Err(ModuleRegistryInitError::UnsupportedEndpoint(
            endpoint.to_string(),
        ))
    } else {
        Ok(PathBuf::from(endpoint))
    }
}

#[derive(Debug, Deserialize)]
struct RegistryIndex {
    #[serde(default)]
    modules: Vec<RegistryIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct RegistryIndexEntry {
    id: String,
    version: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}
