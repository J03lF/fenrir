use std::collections::HashSet;
use std::fmt;

use aes_gcm::aead::{generic_array::GenericArray, Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use chacha20poly1305::aead::{
    generic_array::GenericArray as ChaChaGenericArray, Payload as ChaChaPayload,
};
use chacha20poly1305::XChaCha20Poly1305;
use rand_core::{OsRng, RngCore};

use super::error::CryptoError;
use crate::utils::messages::security::crypto as crypto_messages;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CipherAlgorithm {
    Aes256Gcm,
    XChaCha20Poly1305,
}

impl CipherAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            CipherAlgorithm::Aes256Gcm => "aes-gcm",
            CipherAlgorithm::XChaCha20Poly1305 => "xchacha20-poly1305",
        }
    }

    pub fn key_size(&self) -> usize {
        32
    }

    pub fn nonce_size(&self) -> usize {
        match self {
            CipherAlgorithm::Aes256Gcm => 12,
            CipherAlgorithm::XChaCha20Poly1305 => 24,
        }
    }
}

impl fmt::Display for CipherAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for CipherAlgorithm {
    type Error = CryptoError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "aes-gcm" | "aes256-gcm" | "aes_256_gcm" => Ok(CipherAlgorithm::Aes256Gcm),
            "xchacha20-poly1305" | "xchacha20poly1305" => Ok(CipherAlgorithm::XChaCha20Poly1305),
            other => Err(CryptoError::UnsupportedAlgorithm(
                crypto_messages::unsupported_cipher_algorithm(other),
            )),
        }
    }
}

pub struct AeadRegistry {
    allowed: HashSet<CipherAlgorithm>,
}

impl AeadRegistry {
    pub fn new(allowed: &[CipherAlgorithm]) -> Self {
        Self {
            allowed: allowed.iter().copied().collect(),
        }
    }

    fn ensure_allowed(&self, algorithm: CipherAlgorithm) -> Result<(), CryptoError> {
        if self.allowed.contains(&algorithm) {
            Ok(())
        } else {
            Err(CryptoError::UnsupportedAlgorithm(
                crypto_messages::unsupported_cipher_algorithm(algorithm.as_str()),
            ))
        }
    }

    pub fn encrypt(
        &self,
        algorithm: CipherAlgorithm,
        key: &[u8],
        nonce: &[u8],
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.ensure_allowed(algorithm)?;
        match algorithm {
            CipherAlgorithm::Aes256Gcm => encrypt_aes256_gcm(key, nonce, plaintext, aad),
            CipherAlgorithm::XChaCha20Poly1305 => encrypt_xchacha20(key, nonce, plaintext, aad),
        }
    }

    pub fn decrypt(
        &self,
        algorithm: CipherAlgorithm,
        key: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.ensure_allowed(algorithm)?;
        match algorithm {
            CipherAlgorithm::Aes256Gcm => decrypt_aes256_gcm(key, nonce, ciphertext, aad),
            CipherAlgorithm::XChaCha20Poly1305 => decrypt_xchacha20(key, nonce, ciphertext, aad),
        }
    }

    pub fn generate_nonce(&self, algorithm: CipherAlgorithm) -> Result<Vec<u8>, CryptoError> {
        self.ensure_allowed(algorithm)?;
        let mut nonce = vec![0u8; algorithm.nonce_size()];
        let mut rng = OsRng;
        rng.fill_bytes(&mut nonce);
        Ok(nonce)
    }

    pub fn allowed_algorithms(&self) -> impl Iterator<Item = CipherAlgorithm> + '_ {
        self.allowed.iter().copied()
    }
}

fn encrypt_aes256_gcm(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if key.len() != CipherAlgorithm::Aes256Gcm.key_size() {
        return Err(CryptoError::InvalidKeyLength);
    }
    if nonce.len() != CipherAlgorithm::Aes256Gcm.nonce_size() {
        return Err(CryptoError::InvalidNonceLength);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let nonce = GenericArray::from_slice(nonce);
    cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encryption)
}

fn decrypt_aes256_gcm(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if key.len() != CipherAlgorithm::Aes256Gcm.key_size() {
        return Err(CryptoError::InvalidKeyLength);
    }
    if nonce.len() != CipherAlgorithm::Aes256Gcm.nonce_size() {
        return Err(CryptoError::InvalidNonceLength);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let nonce = GenericArray::from_slice(nonce);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

fn encrypt_xchacha20(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if key.len() != CipherAlgorithm::XChaCha20Poly1305.key_size() {
        return Err(CryptoError::InvalidKeyLength);
    }
    if nonce.len() != CipherAlgorithm::XChaCha20Poly1305.nonce_size() {
        return Err(CryptoError::InvalidNonceLength);
    }
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let nonce = ChaChaGenericArray::from_slice(nonce);
    cipher
        .encrypt(
            nonce,
            ChaChaPayload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Encryption)
}

fn decrypt_xchacha20(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    if key.len() != CipherAlgorithm::XChaCha20Poly1305.key_size() {
        return Err(CryptoError::InvalidKeyLength);
    }
    if nonce.len() != CipherAlgorithm::XChaCha20Poly1305.nonce_size() {
        return Err(CryptoError::InvalidNonceLength);
    }
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::InvalidKeyLength)?;
    let nonce = ChaChaGenericArray::from_slice(nonce);
    cipher
        .decrypt(
            nonce,
            ChaChaPayload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decryption)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_gcm_roundtrip() {
        let registry = AeadRegistry::new(&[CipherAlgorithm::Aes256Gcm]);
        let key = [0x11u8; 32];
        let nonce = registry
            .generate_nonce(CipherAlgorithm::Aes256Gcm)
            .expect("nonce");
        let ciphertext = registry
            .encrypt(
                CipherAlgorithm::Aes256Gcm,
                &key,
                &nonce,
                b"secret data",
                b"aad",
            )
            .expect("encrypt");
        let plaintext = registry
            .decrypt(
                CipherAlgorithm::Aes256Gcm,
                &key,
                &nonce,
                &ciphertext,
                b"aad",
            )
            .expect("decrypt");
        assert_eq!(plaintext, b"secret data");
    }

    #[test]
    fn xchacha_roundtrip() {
        let registry = AeadRegistry::new(&[CipherAlgorithm::XChaCha20Poly1305]);
        let key = [0x22u8; 32];
        let nonce = registry
            .generate_nonce(CipherAlgorithm::XChaCha20Poly1305)
            .expect("nonce");
        let ciphertext = registry
            .encrypt(
                CipherAlgorithm::XChaCha20Poly1305,
                &key,
                &nonce,
                b"top secret",
                b"aad",
            )
            .expect("encrypt");
        let plaintext = registry
            .decrypt(
                CipherAlgorithm::XChaCha20Poly1305,
                &key,
                &nonce,
                &ciphertext,
                b"aad",
            )
            .expect("decrypt");
        assert_eq!(plaintext, b"top secret");
    }
}
