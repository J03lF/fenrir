use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("unsupported cipher algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("invalid key length for selected cipher")]
    InvalidKeyLength,
    #[error("invalid nonce length for selected cipher")]
    InvalidNonceLength,
    #[error("key derivation failed: {0}")]
    Derivation(String),
    #[error("password hashing failed: {0}")]
    PasswordHash(String),
    #[error("encryption failed")]
    Encryption,
    #[error("decryption failed")]
    Decryption,
    #[error("randomness source unavailable: {0}")]
    Random(String),
}

impl From<password_hash::Error> for CryptoError {
    fn from(err: password_hash::Error) -> Self {
        CryptoError::PasswordHash(err.to_string())
    }
}

impl From<rand_core::Error> for CryptoError {
    fn from(err: rand_core::Error) -> Self {
        CryptoError::Random(err.to_string())
    }
}
