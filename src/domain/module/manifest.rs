use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::errors::ModuleError;
use super::id::ModuleId;
use super::serde_helpers;
use super::version::ModuleVersion;

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
