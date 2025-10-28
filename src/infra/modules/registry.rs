use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest::{Certificate, Identity, Url};
use semver::{Version, VersionReq};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task;
use tracing::warn;

use crate::config::ModuleRegistrySection;
use crate::domain::module::{
    ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum, ModuleId,
    ModuleManifest, ModuleRegistryError, ModuleRegistryPort, ModuleSearchQuery,
    ModuleSignatureDescriptor, ModuleSummary, ModuleVersion, SignatureAlgorithm,
};

const DEFAULT_CONTENT_TYPE: &str = "application/gzip";
const USER_AGENT_VALUE: &str = "fenrir-runtime/registry-client";

#[derive(Debug, Clone)]
pub struct HttpModuleRegistry {
    base_url: String,
    client: reqwest::Client,
}

impl HttpModuleRegistry {
    pub fn new(config: &ModuleRegistrySection) -> Result<Self, ModuleRegistryInitError> {
        if config.url.trim().is_empty() {
            return Err(ModuleRegistryInitError::InvalidConfig(
                "modules.registry.url must not be empty".to_string(),
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
                        "modules.registry.auth_token contains invalid characters".to_string(),
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
                ModuleRegistryInitError::InvalidConfig(format!(
                    "Failed to read {}: {}",
                    ca_path, err
                ))
            })?;
            let cert = Certificate::from_pem(&pem).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(format!(
                    "modules.registry.tls.ca_cert_path is not valid PEM: {}",
                    err
                ))
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
                "modules.registry.tls.client_cert_path and client_key_path must be provided together"
                    .to_string(),
            ));
        }

        if let (Some(cert_path), Some(key_path)) = (client_cert.as_ref(), client_key.as_ref()) {
            let cert_pem = fs::read(cert_path).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(format!(
                    "Failed to read client certificate {}: {}",
                    cert_path, err
                ))
            })?;
            let key_pem = fs::read(key_path).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(format!(
                    "Failed to read client key {}: {}",
                    key_path, err
                ))
            })?;
            let mut identity_pem = Vec::with_capacity(cert_pem.len() + key_pem.len() + 1);
            identity_pem.extend_from_slice(&cert_pem);
            if !identity_pem.ends_with(b"\n") {
                identity_pem.push(b'\n');
            }
            identity_pem.extend_from_slice(&key_pem);

            let identity = Identity::from_pem(&identity_pem).map_err(|err| {
                ModuleRegistryInitError::InvalidConfig(format!(
                    "modules.registry.tls.client_cert_path/client_key_path could not be combined into a valid identity: {}",
                    err
                ))
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
            ModuleRegistryError::Unavailable(format!("Invalid registry base URL: {}", err))
        })?;

        base.join(url)
            .map(|joined| joined.to_string())
            .map_err(|err| {
                ModuleRegistryError::Protocol(format!(
                    "Failed to resolve download URL '{}': {}",
                    url, err
                ))
            })
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
                "{}/api/modules/search?q={}",
                base,
                urlencoding::encode(pattern)
            )
        } else {
            format!("{}/api/modules", base)
        };

        let response = self.client.get(&url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(format!("Failed to query registry: {}", err))
        })?;

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(format!(
                "Registry returned status: {}",
                response.status()
            )));
        }

        let modules: Vec<RegistryModule> = response.json().await.map_err(|err| {
            ModuleRegistryError::Protocol(format!("Failed to parse registry response: {}", err))
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
        let url = format!("{}/api/modules/{}", base, urlencoding::encode(id.as_str()));

        let response = self.client.get(&url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(format!("Failed to fetch module: {}", err))
        })?;

        if response.status().as_u16() == 404 {
            return Err(ModuleRegistryError::NotFound {
                module: id.to_string(),
            });
        }

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(format!(
                "Registry returned status: {}",
                response.status()
            )));
        }

        let module: RegistryModule = response.json().await.map_err(|err| {
            ModuleRegistryError::Protocol(format!("Failed to parse module payload: {}", err))
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
                ModuleRegistryError::Protocol(format!("Module {} has no published versions", id))
            })?
        };

        let parsed_version = Version::parse(&target_version.version).map_err(|err| {
            ModuleRegistryError::Protocol(format!("Invalid version string: {}", err))
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
        let download_url = self.resolve_url(&manifest.artifact.download_url)?;
        let response = self.client.get(download_url).send().await.map_err(|err| {
            ModuleRegistryError::Unavailable(format!("Failed to download artifact: {}", err))
        })?;

        if !response.status().is_success() {
            return Err(ModuleRegistryError::Unavailable(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        let archive = response.bytes().await.map_err(|err| {
            ModuleRegistryError::Unavailable(format!("Failed to read artifact: {}", err))
        })?;

        let expected_checksum = hex::decode(&manifest.artifact.checksum.hash).map_err(|err| {
            ModuleRegistryError::Protocol(format!("Invalid checksum encoding: {}", err))
        })?;

        let mut hasher = sha2::Sha256::new();
        hasher.update(&archive);
        let actual_checksum = hasher.finalize();

        if actual_checksum.as_slice() != expected_checksum.as_slice() {
            return Err(ModuleRegistryError::Protocol(
                "Downloaded artifact checksum mismatch".to_string(),
            ));
        }

        let signature_bytes = if manifest.signature.signature.is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(manifest.signature.signature.as_bytes())
                .map_err(|err| {
                    ModuleRegistryError::Protocol(format!("Invalid signature encoding: {}", err))
                })?
        };

        Ok(ModuleBundle {
            manifest: manifest.clone(),
            archive: archive.to_vec(),
            signature: signature_bytes,
            checksum: expected_checksum,
        })
    }
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
                warn!(root = %path.display(), "offline registry root missing, skipping");
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
                        Err(err) => warn!(root = %root.display(), error = %err, "invalid local module manifest"),
                    },
                    Err(err) => warn!(root = %root.display(), error = %err, "failed to load local module manifest"),
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
            .ok_or_else(|| ModuleRegistryError::NotFound { module: needle })
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
        Ok(selected.into_manifest(&module))
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        let module_id = ModuleId::new(&manifest.id)
            .map_err(|err| ModuleRegistryError::Protocol(format!("invalid module id: {err}")))?;
        let module = self.find_module(&module_id).await?;
        let version = module.pick_version(Some(&ModuleVersion(manifest.version.clone())))?;
        version.load_bundle(&module).await
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
            "composite registry requires at least one source"
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
                    warn!(module = %module_id, error = %err, "skipping local module version")
                }
            }
        }

        if versions.is_empty() {
            return Err(LocalManifestError::InvalidField("no valid versions".into()));
        }

        let latest = versions
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version))
            .cloned()
            .ok_or_else(|| LocalManifestError::InvalidField("no versions".into()))?;

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
        let checksum_hex = version
            .checksum
            .ok_or_else(|| LocalManifestError::InvalidField("checksum missing".into()))?;
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

    fn into_manifest(&self, module: &LocalModule) -> ModuleManifest {
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
        let manifest = self.into_manifest(module);
        let artifact_path = self.artifact_path.clone();
        let checksum_hex = self.checksum_hex.clone();
        let signature = self.signature.clone().unwrap_or_default();

        task::spawn_blocking(move || {
            let bytes = std::fs::read(&artifact_path).map_err(|err| {
                ModuleRegistryError::Unavailable(format!(
                    "failed to read artifact {}: {}",
                    artifact_path.display(),
                    err
                ))
            })?;
            let expected = hex::decode(checksum_hex).map_err(|err| {
                ModuleRegistryError::Protocol(format!("invalid checksum hex: {err}"))
            })?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let digest = hasher.finalize();
            if digest.as_slice() != expected.as_slice() {
                return Err(ModuleRegistryError::Protocol(format!(
                    "checksum mismatch for artifact {}",
                    artifact_path.display()
                )));
            }
            let signature_bytes = if signature.is_empty() {
                Vec::new()
            } else {
                BASE64.decode(signature.as_bytes()).map_err(|err| {
                    ModuleRegistryError::Protocol(format!("invalid signature encoding: {err}"))
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
            LocalManifestError::Io(path, err) => {
                write!(f, "cannot read {}: {}", path.display(), err)
            }
            LocalManifestError::Parse(path, err) => {
                write!(f, "cannot parse {}: {}", path.display(), err)
            }
            LocalManifestError::InvalidField(msg) => write!(f, "invalid manifest: {msg}"),
            LocalManifestError::MissingArtifact(path) => {
                write!(f, "missing artifact at {}", path.display())
            }
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
            ModuleRegistryInitError::InvalidConfig(format!(
                "Environment variable {} referenced in modules.registry.auth_token is not set",
                env.trim()
            ))
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
            return Err(ModuleRegistryInitError::InvalidConfig(format!(
                "{} must not resolve to an empty value",
                field
            )));
        }
        Ok(Some(trimmed.to_string()))
    } else {
        Ok(None)
    }
}
