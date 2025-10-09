use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest::Url;
use semver::{Version, VersionReq};
use serde::Deserialize;
use sha2::Digest;
use std::time::Duration;

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

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
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
