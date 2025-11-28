use std::error::Error as StdError;
use std::fmt;

use crate::utils::messages::security::crypto as crypto_messages;

#[derive(Debug)]
pub enum CryptoError {
    UnsupportedAlgorithm(String),
    InvalidKeyLength,
    InvalidNonceLength,
    Derivation(String),
    PasswordHash(String),
    Encryption,
    Decryption,
    Random(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::UnsupportedAlgorithm(msg) => f.write_str(msg),
            CryptoError::InvalidKeyLength => f.write_str(crypto_messages::invalid_key_length()),
            CryptoError::InvalidNonceLength => f.write_str(crypto_messages::invalid_nonce_length()),
            CryptoError::Derivation(reason) => {
                f.write_str(&crypto_messages::derivation_failed(reason))
            }
            CryptoError::PasswordHash(reason) => {
                f.write_str(&crypto_messages::password_hash_failed(reason))
            }
            CryptoError::Encryption => f.write_str(crypto_messages::encryption_failed()),
            CryptoError::Decryption => f.write_str(crypto_messages::decryption_failed()),
            CryptoError::Random(reason) => {
                f.write_str(&crypto_messages::randomness_unavailable(reason))
            }
        }
    }
}

impl StdError for CryptoError {}

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
