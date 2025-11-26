use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{PublicKey, Signature, Verifier};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config::ModuleTrustSection;
use crate::domain::module::{
    ModuleBundle, ModuleVerificationError, ModuleVerifierPort, SignatureAlgorithm,
};

#[derive(Debug, Clone)]
pub struct Ed25519ModuleVerifier {
    require_signature: bool,
    allowed_signers: Option<HashSet<String>>,
    keys: HashMap<String, PublicKey>,
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
                ModuleVerifierInitError::InvalidKeyring(format!("invalid keyring json: {err}"))
            })?;
            for entry in keyring.keys {
                if entry.algorithm.to_ascii_lowercase() != "ed25519" {
                    return Err(ModuleVerifierInitError::UnsupportedAlgorithm(
                        entry.algorithm,
                    ));
                }
                let decoded = BASE64.decode(entry.public_key.trim()).map_err(|err| {
                    ModuleVerifierInitError::InvalidKeyring(format!(
                        "public key {} invalid base64: {err}",
                        entry.id
                    ))
                })?;
                let verifying =
                    PublicKey::from_bytes(decoded.as_slice().try_into().map_err(|_| {
                        ModuleVerifierInitError::InvalidKeyring(format!(
                            "public key {} must be 32 bytes",
                            entry.id
                        ))
                    })?)
                    .map_err(|err| {
                        ModuleVerifierInitError::InvalidKeyring(format!(
                            "public key {} invalid: {err}",
                            entry.id
                        ))
                    })?;
                let key_id = entry.id.to_ascii_lowercase();
                keys.insert(key_id, verifying);
            }
        }
        if config.require_signature && keys.is_empty() {
            return Err(ModuleVerifierInitError::InvalidKeyring(
                "no verifying keys loaded but signatures are required".to_string(),
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
                    "allowlisted signers missing from keyring - signature verification will be skipped for them"
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
                return Err(ModuleVerificationError::Signature(format!(
                    "signer {signer} is not in allowlist"
                )));
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
            return Err(ModuleVerificationError::Checksum(format!(
                "checksum mismatch for module {}",
                bundle.manifest.id
            )));
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
                "signature is required but empty".to_string(),
            ));
        }

        let signer_key = signer.to_ascii_lowercase();
        let verifying_key = match self.keys.get(&signer_key) {
            Some(key) => key,
            None if self.require_signature => {
                return Err(ModuleVerificationError::Signature(format!(
                    "missing verifying key for signer {signer}"
                )))
            }
            None => {
                // No key found, but signature is not empty - this is an error
                return Err(ModuleVerificationError::Signature(format!(
                    "signature provided but no verifying key for signer {signer}"
                )));
            }
        };
        
        // Verify the signature
        if bundle.signature.len() != 64 {
            return Err(ModuleVerificationError::Signature(format!(
                "signature must be 64 bytes, got {} bytes",
                bundle.signature.len()
            )));
        }
        
        let signature =
            Signature::from_bytes(bundle.signature.as_slice().try_into().map_err(|_| {
                ModuleVerificationError::Signature("signature must be 64 bytes".to_string())
            })?)
            .map_err(|err| ModuleVerificationError::Signature(err.to_string()))?;
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
