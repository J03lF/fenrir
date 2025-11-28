use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest::{Certificate, Identity, Url};
use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tracing::warn;

use crate::config::ModuleRegistrySection;
use crate::domain::module::{
    ChecksumAlgorithm, DistributionTarget, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum,
    ModuleId, ModuleManifest, ModuleProgress, ModuleRegistryError, ModuleRegistryPort,
    ModuleSearchQuery, ModuleSignatureDescriptor, ModuleSummary, ModuleVersion, ProgressCallback,
    SignatureAlgorithm,
};
use crate::utils::messages;

const DEFAULT_CONTENT_TYPE: &str = "application/gzip";
const USER_AGENT_VALUE: &str = "fenrir-runtime/registry-client";

#[derive(Debug, Deserialize)]
struct CompatibilityTreeResponse {
    #[allow(dead_code)]
    fenrir_version: String,
    tree: Value,
}

#[derive(Debug, Deserialize)]
struct CompatibilityNodeDto {
    id: String,
    #[allow(dead_code)]
    label: String,
    #[serde(default)]
    children: Vec<CompatibilityNodeDto>,
}

#[derive(Debug, Clone)]
pub struct HttpModuleRegistry {
    base_url: String,
    client: reqwest::Client,
}

impl HttpModuleRegistry {
    pub fn new(config: &ModuleRegistrySection) -> Result<Self, ModuleRegistryInitError> {
        if config.url.trim().is_empty() {
            return Err(ModuleRegistryInitError::InvalidConfig(
                messages::infra::modules::registry::URL_EMPTY.to_string(),
            ));
        }

        let base_url = config.url.trim_end_matches('/').to_string();
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));

        if let Some(ref raw_token) = config.auth_token {
            let token = resolve_secret(raw_token)?;
            if !token.is_empty() {
                let value = HeaderValue::from_str(&format!("Bearer {}", token)).map_err(|_| {
                    ModuleRegistryInitError::InvalidConfig(
                        messages::infra::modules::registry::AUTH_TOKEN_INVALID_CHARS.to_string(),
                    )
                })?;
                headers.insert(AUTHORIZATION, value);
            }
        }

        let mut client_builder = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30));

        if let Some(ca_path) = resolve_optional_path(
            &config.tls.ca_cert_path,
            "modules.registry.tls.ca_cert_path",
        )? {
            let pem = fs::read(&ca_path).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(
                    messages::infra::modules::registry::ca_cert_read_failed(
                        Path::new(&ca_path).display(),
                        err,
                    ),
                )
            })?;
            let cert = Certificate::from_pem(&pem).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(
                    messages::infra::modules::registry::ca_cert_invalid_pem(err),
                )
            })?;
            client_builder = client_builder.add_root_certificate(cert);
        }

        let client_cert = resolve_optional_path(
            &config.tls.client_cert_path,
            "modules.registry.tls.client_cert_path",
        )?;
        let client_key = resolve_optional_path(
            &config.tls.client_key_path,
            "modules.registry.tls.client_key_path",
        )?;

        if client_cert.is_some() ^ client_key.is_some() {
            return Err(ModuleRegistryInitError::InvalidConfig(
                messages::infra::modules::registry::CLIENT_CERT_KEY_MISMATCH.to_string(),
            ));
        }

        if let (Some(cert_path), Some(key_path)) = (client_cert.as_ref(), client_key.as_ref()) {
            let cert_pem = fs::read(cert_path).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(
                    messages::infra::modules::registry::client_cert_read_failed(
                        Path::new(cert_path).display(),
                        err,
                    ),
                )
            })?;
            let key_pem = fs::read(key_path).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(
                    messages::infra::modules::registry::client_key_read_failed(
                        Path::new(key_path).display(),
                        err,
                    ),
                )
            })?;
            let mut identity_pem = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
            identity_pem.extend_from_slice(&cert_pem);
            if !identity_pem.ends_with(b"\n") {
                identity_pem.push(b'\n');
            }
            identity_pem.extend_from_slice(&key_pem);

            let identity = Identity::from_pem(&identity_pem).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(
                    messages::infra::modules::registry::client_identity_build_failed(err),
                )
            })?;
            client_builder = client_builder.identity(identity);
        }

        if config.tls.accept_invalid_certs {
            client_builder = client_builder.danger_accept_invalid_certs(true);
        }

        let client = client_builder
            .build()
            .map_err(|err| ModuleRegistryInitError::InvalidConfig(err.to_string()))?;

        Ok(Self { base_url, client })
    }

    fn resolve_url(&self, url: &str) -> Result<String, ModuleRegistryError> {
        if url.starts_with("http://") || url.starts_with("https://") {
            return Ok(url.to_string());
        }

        let base = Url::parse(&self.base_url).map_err(|err| {
            ModuleRegistryError::Unavailable(messages::infra::modules::registry::base_url_invalid(
                err,
            ))
        })?;

        base.join(url)
            .map(|joined| joined.to_string())
            .map_err(|err| {
                ModuleRegistryError::Protocol(
                    messages::infra::modules::registry::download_url_resolve_failed(url, err),
                )
            })
    }

    async fn download_with_refresh(
        &self,
        manifest: &ModuleManifest,
        allow_refresh: bool,
        progress: Option<ProgressCallback>,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        use futures::StreamExt;

        let mut current_manifest = manifest.clone();
        let mut retried_same = false;
        let mut refreshed_manifest = false;

        loop {
            let download_url = self.resolve_url(&current_manifest.artifact.download_url)?;

            if let Some(ref callback) = progress {
                callback(ModuleProgress::DownloadStarted {
                    module_id: current_manifest.id.clone(),
                    total_bytes: None,
                });
            }

            let response = self.client.get(download_url).send().await.map_err(|err| {
                ModuleRegistryError::Unavailable(
                    messages::infra::modules::registry::artifact_download_failed(err),
                )
            })?;

            if !response.status().is_success() {
                return Err(ModuleRegistryError::Unavailable(
                    messages::infra::modules::registry::download_status_failed(response.status()),
                ));
            }

            let total_bytes = response.content_length();
            let mut downloaded_bytes = 0u64;
            let mut archive_data = Vec::new();

            if let Some(total) = total_bytes {
                archive_data.reserve(total as usize);
            }

            let mut stream = response.bytes_stream();

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result.map_err(|err| {
                    ModuleRegistryError::Unavailable(
                        messages::infra::modules::registry::download_chunk_failed(err),
                    )
                })?;

                archive_data.extend_from_slice(&chunk);
                downloaded_bytes += chunk.len() as u64;

                if let Some(ref callback) = progress {
                    callback(ModuleProgress::DownloadProgress {
                        module_id: current_manifest.id.clone(),
                        downloaded_bytes,
                        total_bytes,
                    });
                }
            }

            if let Some(ref callback) = progress {
                callback(ModuleProgress::DownloadCompleted {
                    module_id: current_manifest.id.clone(),
                    total_bytes: downloaded_bytes,
                });
            }

            let expected_checksum =
                hex::decode(&current_manifest.artifact.checksum.hash).map_err(|err| {
                    ModuleRegistryError::Protocol(
                        messages::infra::modules::registry::checksum_invalid_encoding(err),
                    )
                })?;

            let mut hasher = sha2::Sha256::new();
            hasher.update(&archive_data);
            let actual_checksum = hasher.finalize();

            if actual_checksum.as_slice() != expected_checksum.as_slice() {
                tracing::warn!(
                    module = %current_manifest.id,
                    version = %current_manifest.version,
                    expected = %current_manifest.artifact.checksum.hash,
                    actual = %hex::encode(actual_checksum),
                    "{}",
                    messages::infra::modules::registry::CHECKSUM_MISMATCH_RETRY
                );

                if allow_refresh && !retried_same {
                    retried_same = true;
                    tracing::info!(
                        module = %current_manifest.id,
                        version = %current_manifest.version,
                        "{}",
                        messages::infra::modules::registry::CHECKSUM_MISMATCH_RETRY_ONCE
                    );
                    continue;
                }

                if allow_refresh && !refreshed_manifest {
                    if let Ok(module_id) = ModuleId::new(&current_manifest.id) {
                        let module_version = ModuleVersion(current_manifest.version.clone());
                        match self.fetch_manifest(&module_id, Some(&module_version)).await {
                            Ok(fresh_manifest) => {
                                let checksum_changed = fresh_manifest.artifact.checksum.hash
                                    != current_manifest.artifact.checksum.hash;
                                let url_changed = fresh_manifest.artifact.download_url
                                    != current_manifest.artifact.download_url;
                                if checksum_changed || url_changed {
                                    tracing::info!(
                                        module = %fresh_manifest.id,
                                        version = %fresh_manifest.version,
                                        "{}",
                                        messages::infra::modules::registry::CHECKSUM_MISMATCH_MANIFEST_REFRESH
                                    );
                                    current_manifest = fresh_manifest;
                                    refreshed_manifest = true;
                                    retried_same = true;
                                    continue;
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    module = %current_manifest.id,
                                    version = %current_manifest.version,
                                    error = %err,
                                    "{}",
                                    messages::infra::modules::registry::CHECKSUM_REFRESH_FAILED
                                );
                            }
                        }
                    }
                }

                return Err(ModuleRegistryError::Protocol(
                    messages::infra::modules::registry::CHECKSUM_MISMATCH_FATAL.to_string(),
                ));
            }

            let signature_bytes = if current_manifest.signature.signature.is_empty() {
                Vec::new()
            } else {
                BASE64
                    .decode(current_manifest.signature.signature.as_bytes())
                    .map_err(|err| {
                        ModuleRegistryError::Protocol(
                            messages::infra::modules::registry::signature_invalid_encoding(err),
                        )
                    })?
            };

            return Ok(ModuleBundle {
                manifest: current_manifest.clone(),
                archive: archive_data,
                signature: signature_bytes,
                checksum: expected_checksum,
            });
        }
    }
}

#[async_trait]
impl ModuleRegistryPort for HttpModuleRegistry {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError> {
        let base = self.base_url.trim_end_matches('/');
        let url = if let Some(ref pattern) = query.pattern {
            format!(
                "{}/v1/modules/search?q={}",
                base,
                urlencoding::encode(pattern)
            )
        } else {
            format!("{}/v1/modules", base)
        };

        let response = self.client.get(&url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::registry_query_failed(err),
            )
        })?;

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::registry_status_failed(response.status()),
            ));
        }

        let modules: Vec<RegistryModule> = response.json().await.map_err(|err| {
            ModuleRegistryError::Protocol(
                messages::infra::modules::registry::registry_response_parse_failed(err),
            )
        })?;

        let summaries = modules
            .into_iter()
            .filter_map(|module| {
                let id = ModuleId::new(&module.module.id).ok()?;
                let latest = module
                    .latest_version
                    .or_else(|| module.versions.first().map(|v| v.version.clone()))?;
                let version = ModuleVersion::parse(latest).ok()?;
                Some(ModuleSummary {
                    id,
                    version,
                    title: Some(module.module.name),
                    description: module.module.description,
                    tags: vec![],
                })
            })
            .collect();

        Ok(summaries)
    }

    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{}/v1/modules/{}", base, urlencoding::encode(id.as_str()));

        let response = self.client.get(&url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::module_fetch_failed(err),
            )
        })?;

        if response.status().as_u16() == 404 {
            return Err(ModuleRegistryError::NotFound {
                module: id.to_string(),
            });
        }

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::registry_status_failed(response.status()),
            ));
        }

        let module: RegistryModule = response.json().await.map_err(|err| {
            ModuleRegistryError::Protocol(
                messages::infra::modules::registry::module_payload_parse_failed(err),
            )
        })?;

        let target_version = if let Some(requested) = version {
            module
                .versions
                .iter()
                .find(|ver| ver.version == requested.to_string())
                .ok_or_else(|| ModuleRegistryError::NotFound {
                    module: format!("{} v{}", id, requested),
                })?
        } else {
            module.versions.first().ok_or_else(|| {
                ModuleRegistryError::Protocol(
                    messages::infra::modules::registry::module_no_versions(id),
                )
            })?
        };

        let parsed_version = Version::parse(&target_version.version).map_err(|err| {
            ModuleRegistryError::Protocol(messages::infra::modules::registry::version_invalid(err))
        })?;

        let fenrir_req = parse_fenrir_version_req(
            target_version.fenrir_min_version.as_deref(),
            target_version.fenrir_max_version.as_deref(),
        );

        let authors = module
            .module
            .author
            .as_ref()
            .map(|author| vec![author.clone()])
            .unwrap_or_default();

        let signature = ModuleSignatureDescriptor {
            algorithm: SignatureAlgorithm::Ed25519,
            key_id: module
                .module
                .author
                .clone()
                .unwrap_or_else(|| "module-registry".to_string()),
            signature: target_version.signature.clone().unwrap_or_default(),
        };

        Ok(ModuleManifest {
            id: module.module.id.clone(),
            version: parsed_version,
            title: Some(module.module.name.clone()),
            description: module.module.description.clone(),
            fenrir_version: fenrir_req,
            authors,
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: target_version.download_url.clone(),
                checksum: ModuleChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    hash: target_version.checksum.clone(),
                },
                content_type: Some(DEFAULT_CONTENT_TYPE.to_string()),
                size_bytes: None,
            },
            signature,
            tags: vec![],
            published_at: (target_version.released_at >= 0)
                .then_some(target_version.released_at as u64),
        })
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        self.download_with_refresh(manifest, true, None).await
    }

    async fn download_with_progress(
        &self,
        manifest: &ModuleManifest,
        progress: Option<ProgressCallback>,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        self.download_with_refresh(manifest, true, progress).await
    }

    async fn distribution_targets(
        &self,
        fenrir_version: &str,
    ) -> Result<Vec<DistributionTarget>, ModuleRegistryError> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!(
            "{}/v1/compatibility/{}",
            base,
            urlencoding::encode(fenrir_version)
        );

        let response = self.client.get(&url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::compatibility_query_failed(err),
            )
        })?;

        if response.status().as_u16() == 404 {
            return Ok(Vec::new());
        }

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(
                messages::infra::modules::registry::compatibility_status_failed(response.status()),
            ));
        }

        let payload: CompatibilityTreeResponse = response.json().await.map_err(|err| {
            ModuleRegistryError::Protocol(
                messages::infra::modules::registry::compatibility_response_invalid(err),
            )
        })?;

        if payload.tree.is_null() {
            return Ok(Vec::new());
        }

        let root: CompatibilityNodeDto = serde_json::from_value(payload.tree).map_err(|err| {
            ModuleRegistryError::Protocol(
                messages::infra::modules::registry::compatibility_tree_invalid(err),
            )
        })?;

        let mut targets = HashMap::new();
        collect_distribution_targets(&root, &mut targets);

        Ok(targets
            .into_iter()
            .map(|(module_id, version)| DistributionTarget { module_id, version })
            .collect())
    }
}

fn collect_distribution_targets(
    node: &CompatibilityNodeDto,
    acc: &mut HashMap<ModuleId, ModuleVersion>,
) {
    if let Some((module_id, version)) = parse_module_from_node(node) {
        acc.entry(module_id).or_insert(version);
    }
    for child in &node.children {
        collect_distribution_targets(child, acc);
    }
}

fn parse_module_from_node(node: &CompatibilityNodeDto) -> Option<(ModuleId, ModuleVersion)> {
    let base_id = node.id.split(':').next().unwrap_or(&node.id);
    let (module_raw, version_raw) = base_id.split_once('@')?;
    let module = ModuleId::new(module_raw).ok()?;
    let version = ModuleVersion::parse(version_raw).ok()?;
    Some((module, version))
}

#[derive(Debug, thiserror::Error)]
pub enum ModuleRegistryInitError {
    #[error("Configuration error: {0}")]
    InvalidConfig(String),
}

#[derive(Clone)]
pub struct LocalModuleRegistry {
    roots: Arc<Vec<PathBuf>>,
}

impl LocalModuleRegistry {
    pub fn try_new(config: &ModuleRegistrySection) -> Option<Self> {
        if config.offline_dirs.is_empty() {
            return None;
        }

        let mut valid = Vec::new();
        for raw in &config.offline_dirs {
            if raw.trim().is_empty() {
                continue;
            }
            let path = PathBuf::from(raw);
            if path.exists() {
                valid.push(path);
            } else {
                warn!(
                    root = %path.display(),
                    "{}",
                    messages::infra::modules::registry::OFFLINE_ROOT_MISSING
                );
            }
        }

        if valid.is_empty() {
            None
        } else {
            Some(Self {
                roots: Arc::new(valid),
            })
        }
    }

    async fn load_modules(&self) -> Result<Vec<LocalModule>, ModuleRegistryError> {
        let roots = Arc::clone(&self.roots);
        task::spawn_blocking(move || {
            let mut modules = Vec::new();
            for root in roots.iter() {
                match LocalManifestFile::load(root) {
                    Ok(file) => match LocalModule::from_manifest(root, file) {
                        Ok(module) => modules.push(module),
                        Err(err) => warn!(
                            root = %root.display(),
                            error = %err,
                            "{}",
                            messages::infra::modules::registry::LOCAL_MANIFEST_INVALID
                        ),
                    },
                    Err(err) => warn!(
                        root = %root.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::registry::LOCAL_MANIFEST_LOAD_FAILED
                    ),
                }
            }
            Ok(modules)
        })
        .await
        .map_err(|err| ModuleRegistryError::Unavailable(err.to_string()))?
    }

    async fn find_module(&self, id: &ModuleId) -> Result<LocalModule, ModuleRegistryError> {
        let needle = id.to_string();
        let modules = self.load_modules().await?;
        modules
            .into_iter()
            .find(|module| module.id.to_string() == needle)
            .ok_or(ModuleRegistryError::NotFound { module: needle })
    }
}

#[async_trait]
impl ModuleRegistryPort for LocalModuleRegistry {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError> {
        let modules = self.load_modules().await?;
        let pattern = query.pattern.map(|p| p.to_ascii_lowercase());
        let mut summaries = Vec::new();

        for module in modules {
            if let Some(ref pat) = pattern {
                let matches_id = module.id.to_string().contains(pat);
                let matches_title = module.title.to_ascii_lowercase().contains(pat);
                if !matches_id && !matches_title {
                    continue;
                }
            }

            summaries.push(ModuleSummary {
                id: module.id.clone(),
                version: module.latest.version.clone(),
                title: Some(module.title.clone()),
                description: module.description.clone(),
                tags: module.tags.clone(),
            });
        }

        Ok(summaries)
    }

    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError> {
        let module = self.find_module(id).await?;
        let selected = module.pick_version(version)?;
        Ok(selected.as_manifest(&module))
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        let module_id = ModuleId::new(&manifest.id).map_err(|err| {
            ModuleRegistryError::Protocol(messages::infra::modules::registry::module_id_invalid(
                err,
            ))
        })?;
        let module = self.find_module(&module_id).await?;
        let version = module.pick_version(Some(&ModuleVersion(manifest.version.clone())))?;
        version.load_bundle(&module).await
    }

    async fn download_with_progress(
        &self,
        manifest: &ModuleManifest,
        _progress: Option<ProgressCallback>,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        // Local downloads are instant, no progress needed
        self.download(manifest).await
    }

    async fn distribution_targets(
        &self,
        fenrir_version: &str,
    ) -> Result<Vec<DistributionTarget>, ModuleRegistryError> {
        let parsed = Version::parse(fenrir_version).map_err(|err| {
            ModuleRegistryError::Protocol(
                messages::infra::modules::registry::fenrir_version_invalid(err),
            )
        })?;
        let modules = self.load_modules().await?;
        let mut targets = Vec::new();
        for module in modules {
            if let Some(entry) = module
                .versions
                .iter()
                .filter(|version| version.is_compatible_with(&parsed))
                .max_by(|a, b| a.version.cmp(&b.version))
            {
                targets.push(DistributionTarget {
                    module_id: module.id.clone(),
                    version: entry.version.clone(),
                });
            }
        }
        Ok(targets)
    }
}

#[derive(Clone)]
pub struct CompositeModuleRegistry {
    sources: Vec<Arc<dyn ModuleRegistryPort>>,
}

impl CompositeModuleRegistry {
    pub fn new(mut sources: Vec<Arc<dyn ModuleRegistryPort>>) -> Self {
        sources.retain(|_| true);
        debug_assert!(
            !sources.is_empty(),
            "{}",
            messages::infra::modules::registry::COMPOSITE_NO_SOURCES
        );
        Self { sources }
    }
}

#[async_trait]
impl ModuleRegistryPort for CompositeModuleRegistry {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError> {
        let mut seen = HashSet::new();
        let mut results = Vec::new();

        for source in &self.sources {
            let entries = source.search(query.clone()).await?;
            for summary in entries {
                let key = summary.id.to_string();
                if seen.insert(key) {
                    results.push(summary);
                }
            }
        }

        Ok(results)
    }

    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError> {
        for source in &self.sources {
            match source.fetch_manifest(id, version).await {
                Ok(manifest) => return Ok(manifest),
                Err(ModuleRegistryError::NotFound { .. }) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(ModuleRegistryError::NotFound {
            module: id.to_string(),
        })
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        for source in &self.sources {
            match source.download(manifest).await {
                Ok(bundle) => return Ok(bundle),
                Err(ModuleRegistryError::NotFound { .. }) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(ModuleRegistryError::NotFound {
            module: manifest.id.clone(),
        })
    }

    async fn download_with_progress(
        &self,
        manifest: &ModuleManifest,
        progress: Option<ProgressCallback>,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        for source in &self.sources {
            match source
                .download_with_progress(manifest, progress.clone())
                .await
            {
                Ok(bundle) => return Ok(bundle),
                Err(ModuleRegistryError::NotFound { .. }) => continue,
                Err(err) => return Err(err),
            }
        }
        Err(ModuleRegistryError::NotFound {
            module: manifest.id.clone(),
        })
    }

    async fn distribution_targets(
        &self,
        fenrir_version: &str,
    ) -> Result<Vec<DistributionTarget>, ModuleRegistryError> {
        let mut combined: HashMap<ModuleId, ModuleVersion> = HashMap::new();
        for source in &self.sources {
            match source.distribution_targets(fenrir_version).await {
                Ok(targets) => {
                    for entry in targets {
                        combined
                            .entry(entry.module_id.clone())
                            .or_insert(entry.version.clone());
                    }
                }
                Err(ModuleRegistryError::NotFound { .. }) => continue,
                Err(err) => return Err(err),
            }
        }

        Ok(combined
            .into_iter()
            .map(|(module_id, version)| DistributionTarget { module_id, version })
            .collect())
    }
}

#[derive(Debug, Clone)]
struct LocalModule {
    id: ModuleId,
    title: String,
    description: Option<String>,
    author: Vec<String>,
    tags: Vec<String>,
    versions: Vec<LocalModuleVersion>,
    latest: LocalModuleVersion,
}

impl LocalModule {
    fn from_manifest(root: &Path, manifest: LocalManifestFile) -> Result<Self, LocalManifestError> {
        let module_id = ModuleId::new(&manifest.id)
            .map_err(|err| LocalManifestError::InvalidField(err.to_string()))?;
        let title = manifest.name.unwrap_or_else(|| manifest.id.clone());
        let author = manifest.author.map(|a| vec![a]).unwrap_or_default();
        let tags = manifest.tags.unwrap_or_default();

        let mut versions = Vec::new();
        for version in manifest.versions {
            match LocalModuleVersion::from_entry(root, &manifest.id, version) {
                Ok(entry) => versions.push(entry),
                Err(err) => {
                    warn!(
                        module = %module_id,
                        error = %err,
                        "{}",
                        messages::infra::modules::registry::LOCAL_VERSION_SKIP
                    )
                }
            }
        }

        if versions.is_empty() {
            return Err(LocalManifestError::InvalidField(
                messages::infra::modules::registry::LOCAL_MANIFEST_NO_VALID_VERSIONS.into(),
            ));
        }

        let latest = versions
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .cloned()
            .ok_or_else(|| {
                LocalManifestError::InvalidField(
                    messages::infra::modules::registry::LOCAL_MANIFEST_NO_VERSIONS.into(),
                )
            })?;

        Ok(Self {
            id: module_id,
            title,
            description: manifest.description,
            author,
            tags,
            versions,
            latest,
        })
    }

    fn pick_version(
        &self,
        version: Option<&ModuleVersion>,
    ) -> Result<LocalModuleVersion, ModuleRegistryError> {
        if let Some(requested) = version {
            self.versions
                .iter()
                .find(|entry| &entry.version == requested)
                .cloned()
                .ok_or_else(|| ModuleRegistryError::NotFound {
                    module: format!("{}@{}", self.id, requested),
                })
        } else {
            Ok(self.latest.clone())
        }
    }
}

#[derive(Debug, Clone)]
struct LocalModuleVersion {
    version: ModuleVersion,
    artifact_path: PathBuf,
    artifact_url: String,
    checksum_hex: String,
    published_at: Option<u64>,
    fenrir_req: Option<VersionReq>,
    signature: Option<String>,
    signer: Option<String>,
}

impl LocalModuleVersion {
    fn is_compatible_with(&self, version: &Version) -> bool {
        match &self.fenrir_req {
            Some(req) => req.matches(version),
            None => true,
        }
    }
}

impl LocalModuleVersion {
    fn from_entry(
        root: &Path,
        module_id: &str,
        version: LocalManifestVersion,
    ) -> Result<Self, LocalManifestError> {
        let parsed_version = ModuleVersion::parse(&version.version)
            .map_err(|err| LocalManifestError::InvalidField(err.to_string()))?;
        let artifact_url = version.artifact_url.unwrap_or_default();
        let file_name = artifact_file_name(&artifact_url, module_id, &version.version);
        let artifact_path = root.join(file_name);
        if !artifact_path.exists() {
            return Err(LocalManifestError::MissingArtifact(artifact_path));
        }
        let checksum_hex = version.checksum.ok_or_else(|| {
            LocalManifestError::InvalidField(
                messages::infra::modules::registry::LOCAL_MANIFEST_CHECKSUM_MISSING.into(),
            )
        })?;
        let fenrir_req =
            parse_fenrir_version_req(version.fenrir_min.as_deref(), version.fenrir_max.as_deref());
        let published_at = version.released.and_then(|raw| parse_timestamp(&raw).ok());

        Ok(Self {
            version: parsed_version,
            artifact_path,
            artifact_url,
            checksum_hex,
            published_at,
            fenrir_req,
            signature: version.signature,
            signer: version.signer,
        })
    }

    fn as_manifest(&self, module: &LocalModule) -> ModuleManifest {
        ModuleManifest {
            id: module.id.to_string(),
            version: self.version.0.clone(),
            title: Some(module.title.clone()),
            description: module.description.clone(),
            fenrir_version: self.fenrir_req.clone(),
            authors: module.author.clone(),
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: self.artifact_url.clone(),
                checksum: ModuleChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    hash: self.checksum_hex.clone(),
                },
                content_type: Some(DEFAULT_CONTENT_TYPE.to_string()),
                size_bytes: None,
            },
            signature: ModuleSignatureDescriptor {
                algorithm: SignatureAlgorithm::Ed25519,
                key_id: self
                    .signer
                    .clone()
                    .unwrap_or_else(|| "local-dev".to_string()),
                signature: self.signature.clone().unwrap_or_default(),
            },
            tags: module.tags.clone(),
            published_at: self.published_at,
        }
    }

    async fn load_bundle(&self, module: &LocalModule) -> Result<ModuleBundle, ModuleRegistryError> {
        let manifest = self.as_manifest(module);
        let artifact_path = self.artifact_path.clone();
        let checksum_hex = self.checksum_hex.clone();
        let signature = self.signature.clone().unwrap_or_default();

        task::spawn_blocking(move || {
            let bytes = std::fs::read(&artifact_path).map_err(|err| {
                ModuleRegistryError::Unavailable(
                    messages::infra::modules::registry::artifact_read_failed(
                        artifact_path.display(),
                        err,
                    ),
                )
            })?;
            let expected = hex::decode(checksum_hex).map_err(|err| {
                ModuleRegistryError::Protocol(
                    messages::infra::modules::registry::artifact_checksum_invalid_hex(err),
                )
            })?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let digest = hasher.finalize();
            if digest.as_slice() != expected.as_slice() {
                return Err(ModuleRegistryError::Protocol(
                    messages::infra::modules::registry::artifact_checksum_mismatch(
                        artifact_path.display(),
                    ),
                ));
            }
            let signature_bytes = if signature.is_empty() {
                Vec::new()
            } else {
                BASE64.decode(signature.as_bytes()).map_err(|err| {
                    ModuleRegistryError::Protocol(
                        messages::infra::modules::registry::artifact_signature_invalid_encoding(
                            err,
                        ),
                    )
                })?
            };
            Ok(ModuleBundle {
                manifest,
                archive: bytes,
                signature: signature_bytes,
                checksum: expected,
            })
        })
        .await
        .map_err(|err| ModuleRegistryError::Unavailable(err.to_string()))?
    }
}

#[derive(Debug, Deserialize)]
struct LocalManifestFile {
    id: String,
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    tags: Option<Vec<String>>,
    #[serde(default)]
    versions: Vec<LocalManifestVersion>,
}

impl LocalManifestFile {
    fn load(root: &Path) -> Result<Self, LocalManifestError> {
        let manifest_path = root.join("module-manifest.toml");
        let contents = std::fs::read_to_string(&manifest_path)
            .map_err(|err| LocalManifestError::Io(manifest_path.clone(), err))?;
        toml::from_str(&contents).map_err(|err| LocalManifestError::Parse(manifest_path, err))
    }
}

#[derive(Debug, Deserialize)]
struct LocalManifestVersion {
    version: String,
    released: Option<String>,
    #[allow(dead_code)]
    release_notes: Option<String>,
    fenrir_min: Option<String>,
    fenrir_max: Option<String>,
    artifact_url: Option<String>,
    checksum: Option<String>,
    #[allow(dead_code)]
    checksum_algorithm: Option<String>,
    signature: Option<String>,
    signer: Option<String>,
}

#[derive(Debug)]
enum LocalManifestError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    InvalidField(String),
    MissingArtifact(PathBuf),
}

impl std::fmt::Display for LocalManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocalManifestError::Io(path, err) => write!(
                f,
                "{}",
                messages::infra::modules::registry::local_manifest_read_failed(path.display(), err)
            ),
            LocalManifestError::Parse(path, err) => write!(
                f,
                "{}",
                messages::infra::modules::registry::local_manifest_parse_failed(
                    path.display(),
                    err
                )
            ),
            LocalManifestError::InvalidField(msg) => write!(
                f,
                "{}",
                messages::infra::modules::registry::local_manifest_invalid_field(msg)
            ),
            LocalManifestError::MissingArtifact(path) => write!(
                f,
                "{}",
                messages::infra::modules::registry::local_manifest_artifact_missing(path.display())
            ),
        }
    }
}

impl std::error::Error for LocalManifestError {}

fn artifact_file_name(artifact_url: &str, module_id: &str, version: &str) -> String {
    if let Some(segment) = artifact_url.split('/').rev().find(|part| !part.is_empty()) {
        segment.to_string()
    } else {
        format!("{}-{}.tar.gz", module_id, version)
    }
}

fn parse_timestamp(raw: &str) -> Result<u64, time::error::Parse> {
    use time::format_description::well_known::Rfc3339;
    let dt = time::OffsetDateTime::parse(raw, &Rfc3339)?;
    Ok(dt.unix_timestamp() as u64)
}

#[derive(Debug, Deserialize)]
struct RegistryModule {
    module: ModuleInfo,
    versions: Vec<VersionInfo>,
    latest_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModuleInfo {
    id: String,
    name: String,
    description: Option<String>,
    author: Option<String>,
    #[allow(dead_code)]
    github_url: Option<String>,
    #[allow(dead_code)]
    created_at: i64,
}

#[derive(Debug, Deserialize)]
struct VersionInfo {
    version: String,
    fenrir_min_version: Option<String>,
    fenrir_max_version: Option<String>,
    download_url: String,
    checksum: String,
    #[allow(dead_code)]
    checksum_algorithm: String,
    signature: Option<String>,
    #[allow(dead_code)]
    release_notes: Option<String>,
    released_at: i64,
}

fn parse_fenrir_version_req(min: Option<&str>, max: Option<&str>) -> Option<VersionReq> {
    match (min, max) {
        (Some(min_v), Some(max_v)) => {
            let req_str = format!(">={}, <{}", min_v, max_v);
            VersionReq::parse(&req_str).ok()
        }
        (Some(min_v), None) => VersionReq::parse(&format!(">={}", min_v)).ok(),
        (None, Some(max_v)) => VersionReq::parse(&format!("<{}", max_v)).ok(),
        (None, None) => None,
    }
}

fn resolve_secret(raw: &str) -> Result<String, ModuleRegistryInitError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if let Some(env) = trimmed.strip_prefix("env:") {
        std::env::var(env.trim()).map_err(|_| {
            ModuleRegistryInitError::InvalidConfig(
                messages::infra::modules::registry::auth_token_env_missing(env.trim()),
            )
        })
    } else {
        Ok(trimmed.to_string())
    }
}

fn resolve_optional_path(
    raw: &Option<String>,
    field: &str,
) -> Result<Option<String>, ModuleRegistryInitError> {
    if let Some(value) = raw {
        let resolved = resolve_secret(value)?;
        let trimmed = resolved.trim();
        if trimmed.is_empty() {
            return Err(ModuleRegistryInitError::InvalidConfig(
                messages::infra::modules::registry::resolved_empty(field),
            ));
        }
        Ok(Some(trimmed.to_string()))
    } else {
        Ok(None)
    }
}
