use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::path::PathBuf;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config::ModuleTrustSection;
use crate::domain::module::{
    ModuleBundle, ModuleVerificationError, ModuleVerifierPort, SignatureAlgorithm,
};
use crate::utils::messages;

#[derive(Debug, Clone)]
pub struct Ed25519ModuleVerifier {
    require_signature: bool,
    allowed_signers: Option<HashSet<String>>,
    keys: HashMap<String, VerifyingKey>,
}

impl Ed25519ModuleVerifier {
    pub fn from_config(config: &ModuleTrustSection) -> Result<Self, ModuleVerifierInitError> {
        let mut keys = HashMap::new();
        if let Some(path) = &config.keyring_path {
            let bytes = std::fs::read(path).map_err(|err| ModuleVerifierInitError::Io {
                path: PathBuf::from(path),
                source: err,
            })?;
            let keyring: Keyring = serde_json::from_slice(&bytes).map_err(|err| {
                ModuleVerifierInitError::InvalidKeyring(
                    messages::infra::modules::verifier::keyring_invalid_json(err),
                )
            })?;
            for entry in keyring.keys {
                if !entry.algorithm.eq_ignore_ascii_case("ed25519") {
                    return Err(ModuleVerifierInitError::UnsupportedAlgorithm(
                        entry.algorithm,
                    ));
                }
                let decoded = BASE64.decode(entry.public_key.trim()).map_err(|err| {
                    ModuleVerifierInitError::InvalidKeyring(
                        messages::infra::modules::verifier::public_key_invalid_base64(
                            &entry.id, err,
                        ),
                    )
                })?;
                if decoded.len() != ed25519_dalek::PUBLIC_KEY_LENGTH {
                    return Err(ModuleVerifierInitError::InvalidKeyring(
                        messages::infra::modules::verifier::public_key_length_invalid(&entry.id),
                    ));
                }
                let mut key_bytes = [0u8; ed25519_dalek::PUBLIC_KEY_LENGTH];
                key_bytes.copy_from_slice(&decoded);
                let verifying = VerifyingKey::from_bytes(&key_bytes).map_err(|err| {
                    ModuleVerifierInitError::InvalidKeyring(
                        messages::infra::modules::verifier::public_key_invalid(&entry.id, err),
                    )
                })?;
                let key_id = entry.id.to_ascii_lowercase();
                keys.insert(key_id, verifying);
            }
        }
        if config.require_signature && keys.is_empty() {
            return Err(ModuleVerifierInitError::InvalidKeyring(
                messages::infra::modules::verifier::NO_KEYS_LOADED.to_string(),
            ));
        }
        let allowed_signers: Option<HashSet<String>> = if config.allowed_signers.is_empty() {
            None
        } else {
            Some(
                config
                    .allowed_signers
                    .iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect(),
            )
        };
        if let Some(allowed) = &allowed_signers {
            let missing: Vec<String> = allowed
                .iter()
                .filter(|signer| !keys.contains_key(*signer))
                .cloned()
                .collect();
            if !missing.is_empty() {
                warn!(
                    missing = ?missing,
                    "{}",
                    messages::infra::modules::verifier::ALLOWLIST_MISSING_KEYS
                );
            }
        }
        Ok(Self {
            require_signature: config.require_signature,
            allowed_signers,
            keys,
        })
    }

    fn ensure_allowed(&self, signer: &str) -> Result<(), ModuleVerificationError> {
        if let Some(allowed) = &self.allowed_signers {
            if !allowed.contains(&signer.to_ascii_lowercase()) {
                return Err(ModuleVerificationError::Signature(
                    messages::infra::modules::verifier::signer_not_in_allowlist(signer),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ModuleVerifierPort for Ed25519ModuleVerifier {
    async fn verify(&self, bundle: &ModuleBundle) -> Result<(), ModuleVerificationError> {
        let mut hasher = Sha256::new();
        hasher.update(&bundle.archive);
        let digest = hasher.finalize();
        if digest.as_slice() != bundle.checksum.as_slice() {
            return Err(ModuleVerificationError::Checksum(
                messages::infra::modules::verifier::checksum_mismatch(&bundle.manifest.id),
            ));
        }

        let signer = bundle.manifest.signature.key_id.trim();
        if bundle.manifest.signature.algorithm != SignatureAlgorithm::Ed25519 {
            return Err(ModuleVerificationError::Unsupported);
        }
        self.ensure_allowed(signer)?;

        // If signature is empty, check if we can skip verification
        if bundle.signature.is_empty() {
            // If require_signature is false, always skip
            if !self.require_signature {
                return Ok(()); // Skip verification for empty signatures when not required
            }

            // If require_signature is true, but signer is in allowlist and no key is found,
            // allow empty signature (for manifest-based modules from registry)
            let signer_key = signer.to_ascii_lowercase();
            if !self.keys.contains_key(&signer_key) {
                // Signer is in allowlist (we passed ensure_allowed), but no key in keyring
                // This is acceptable for manifest-based modules from registry
                return Ok(());
            }

            // Key exists but signature is empty - this is an error
            return Err(ModuleVerificationError::Signature(
                messages::infra::modules::verifier::SIGNATURE_REQUIRED_EMPTY.to_string(),
            ));
        }

        let signer_key = signer.to_ascii_lowercase();
        let verifying_key = match self.keys.get(&signer_key) {
            Some(key) => key,
            None if self.require_signature => {
                return Err(ModuleVerificationError::Signature(
                    messages::infra::modules::verifier::missing_verifier_for_signer(signer),
                ))
            }
            None => {
                // No key found, but signature is not empty - this is an error
                return Err(ModuleVerificationError::Signature(
                    messages::infra::modules::verifier::signature_without_key(signer),
                ));
            }
        };

        // Verify the signature
        if bundle.signature.len() != 64 {
            return Err(ModuleVerificationError::Signature(
                messages::infra::modules::verifier::signature_length_invalid(
                    bundle.signature.len(),
                ),
            ));
        }

        let signature_bytes: [u8; ed25519_dalek::SIGNATURE_LENGTH] =
            bundle.signature.as_slice().try_into().map_err(|_| {
                ModuleVerificationError::Signature(
                    messages::infra::modules::verifier::signature_length_invalid(
                        bundle.signature.len(),
                    ),
                )
            })?;
        let signature = Signature::from(signature_bytes);
        verifying_key
            .verify(digest.as_slice(), &signature)
            .map_err(|err| ModuleVerificationError::Signature(err.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModuleVerifierInitError {
    #[error("cannot read keyring {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("keyring invalid: {0}")]
    InvalidKeyring(String),
    #[error("key algorithm {0} not supported")]
    UnsupportedAlgorithm(String),
}

#[derive(Debug, Deserialize)]
struct Keyring {
    #[serde(rename = "version")]
    _version: u32,
    keys: Vec<KeyringEntry>,
}

#[derive(Debug, Deserialize)]
struct KeyringEntry {
    id: String,
    algorithm: String,
    public_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::module::{
        ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleChecksum, ModuleManifest,
        ModuleSignatureDescriptor,
    };
    use ed25519_dalek::{Signer, SigningKey};

    fn manifest(key_id: &str) -> ModuleManifest {
        ModuleManifest {
            id: "athenos".to_string(),
            version: semver::Version::new(1, 0, 0),
            title: None,
            description: None,
            fenrir_version: None,
            authors: vec![],
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: String::new(),
                checksum: ModuleChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    hash: String::new(),
                },
                content_type: None,
                size_bytes: None,
            },
            signature: ModuleSignatureDescriptor {
                algorithm: SignatureAlgorithm::Ed25519,
                key_id: key_id.to_string(),
                signature: String::new(),
            },
            dependencies: vec![],
            tags: vec![],
            published_at: None,
        }
    }

    /// A registry key signs the archive digest; the verifier accepts exactly
    /// that signature (ed25519-dalek 2, W9A-09).
    #[tokio::test]
    async fn registry_signatures_verify_and_tampering_fails() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let dir = std::env::temp_dir().join(format!("w9-host-keyring-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let keyring = dir.join("keyring.json");
        std::fs::write(
            &keyring,
            serde_json::json!({
                "version": 1,
                "keys": [{
                    "id": "module-registry",
                    "algorithm": "ed25519",
                    "public_key": BASE64.encode(signing.verifying_key().as_bytes()),
                }]
            })
            .to_string(),
        )
        .unwrap();
        let verifier = Ed25519ModuleVerifier::from_config(&ModuleTrustSection {
            require_signature: true,
            allowed_signers: vec!["module-registry".to_string()],
            keyring_path: Some(keyring.display().to_string()),
        })
        .unwrap();

        let archive = b"module archive bytes".to_vec();
        let digest = Sha256::digest(&archive).to_vec();
        let signature = signing.sign(&digest).to_bytes().to_vec();
        let bundle = ModuleBundle {
            manifest: manifest("module-registry"),
            archive: archive.clone(),
            signature: signature.clone(),
            checksum: digest.clone(),
        };
        verifier.verify(&bundle).await.unwrap();

        let mut forged = signature;
        forged[10] ^= 1;
        let tampered = ModuleBundle {
            manifest: manifest("module-registry"),
            archive,
            signature: forged,
            checksum: digest,
        };
        assert!(verifier.verify(&tampered).await.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
