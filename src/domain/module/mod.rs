use async_trait::async_trait;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModuleError> {
        let trimmed = value.into().trim().to_string();
        if trimmed.is_empty() {
            return Err(ModuleError::Validation(
                "module id must not be empty".to_string(),
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(ModuleError::Validation(
                "module id may only contain [a-z0-9-_]".to_string(),
            ));
        }
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleVersion(pub Version);

impl ModuleVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ModuleError> {
        let version = Version::parse(value.as_ref()).map_err(|err| {
            ModuleError::Validation(format!(
                "invalid module version `{}`: {err}",
                value.as_ref()
            ))
        })?;
        Ok(Self(version))
    }

    pub fn as_semver(&self) -> &Version {
        &self.0
    }
}

impl fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for ModuleVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ModuleVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw)
            .map(ModuleVersion)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub id: String,
    #[serde(with = "serde_helpers::version")]
    pub version: Version,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default, with = "serde_helpers::option_version_req")]
    pub fenrir_version: Option<VersionReq>,
    #[serde(default)]
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub artifact: ModuleArtifactDescriptor,
    pub signature: ModuleSignatureDescriptor,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub published_at: Option<u64>,
}

impl ModuleManifest {
    pub fn module_id(&self) -> Result<ModuleId, ModuleError> {
        ModuleId::new(&self.id)
    }

    pub fn module_version(&self) -> ModuleVersion {
        ModuleVersion(self.version.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleArtifactDescriptor {
    pub download_url: String,
    pub checksum: ModuleChecksum,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksumAlgorithm {
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSignatureDescriptor {
    pub algorithm: SignatureAlgorithm,
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureAlgorithm {
    Ed25519,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSearchQuery {
    pub pattern: Option<String>,
}

impl ModuleSearchQuery {
    pub fn new(pattern: Option<String>) -> Self {
        Self { pattern }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleSummary {
    pub id: ModuleId,
    pub version: ModuleVersion,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ModuleBundle {
    pub manifest: ModuleManifest,
    pub archive: Vec<u8>,
    pub signature: Vec<u8>,
    pub checksum: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct InstalledModule {
    pub manifest: ModuleManifest,
    pub installed_at: SystemTime,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleInstallStatus {
    Installed,
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone)]
pub struct ModuleInstallResult {
    pub status: ModuleInstallStatus,
    pub manifest: ModuleManifest,
    pub path: String,
}

#[async_trait]
pub trait ModuleRegistryPort: Send + Sync {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError>;
    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError>;
    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError>;
}

#[async_trait]
pub trait ModuleStoragePort: Send + Sync {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError>;
    async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError>;
    async fn stage_and_activate(
        &self,
        bundle: ModuleBundle,
    ) -> Result<ModuleInstallResult, ModuleStorageError>;
    async fn remove(&self, id: &ModuleId) -> Result<(), ModuleStorageError>;
}

#[async_trait]
pub trait ModuleVerifierPort: Send + Sync {
    async fn verify(&self, bundle: &ModuleBundle) -> Result<(), ModuleVerificationError>;
}

#[derive(thiserror::Error, Debug)]
pub enum ModuleError {
    #[error("validation error: {0}")]
    Validation(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ModuleRegistryError {
    #[error("registry unavailable: {0}")]
    Unavailable(String),
    #[error("module `{module}` wurde nicht gefunden")]
    NotFound { module: String },
    #[error("registry protocol error: {0}")]
    Protocol(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ModuleStorageError {
    #[error("storage unavailable: {0}")]
    Unavailable(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
}

#[derive(thiserror::Error, Debug)]
pub enum ModuleVerificationError {
    #[error("signature verification failed: {0}")]
    Signature(String),
    #[error("checksum verification failed: {0}")]
    Checksum(String),
    #[error("unsupported algorithm")]
    Unsupported,
}

pub type ModuleResult<T> = Result<T, ModuleServiceError>;

#[derive(thiserror::Error, Debug)]
pub enum ModuleServiceError {
    #[error("registry error: {0}")]
    Registry(#[source] ModuleRegistryError),
    #[error("storage error: {0}")]
    Storage(#[source] ModuleStorageError),
    #[error("verification error: {0}")]
    Verification(#[source] ModuleVerificationError),
}

impl From<ModuleRegistryError> for ModuleServiceError {
    fn from(err: ModuleRegistryError) -> Self {
        Self::Registry(err)
    }
}

impl From<ModuleStorageError> for ModuleServiceError {
    fn from(err: ModuleStorageError) -> Self {
        Self::Storage(err)
    }
}

impl From<ModuleVerificationError> for ModuleServiceError {
    fn from(err: ModuleVerificationError) -> Self {
        Self::Verification(err)
    }
}

mod serde_helpers {
    use semver::{Version, VersionReq};
    use serde::{Deserialize, Deserializer, Serializer};

    pub mod option_version_req {
        use super::*;

        pub fn serialize<S>(value: &Option<VersionReq>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match value {
                Some(req) => serializer.serialize_some(&req.to_string()),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<VersionReq>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let raw = Option::<String>::deserialize(deserializer)?;
            match raw {
                Some(value) => VersionReq::parse(&value)
                    .map(Some)
                    .map_err(serde::de::Error::custom),
                None => Ok(None),
            }
        }
    }

    pub mod version {
        use super::*;

        pub fn serialize<S>(value: &Version, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&value.to_string())
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Version, D::Error>
        where
            D: Deserializer<'de>,
        {
            let raw = String::deserialize(deserializer)?;
            Version::parse(&raw).map_err(serde::de::Error::custom)
        }
    }
}
