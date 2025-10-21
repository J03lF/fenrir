use argon2::{
    password_hash::SaltString, Algorithm, Argon2, ParamsBuilder, PasswordHash, PasswordHasher,
    PasswordVerifier, Version,
};
use rand_core::{OsRng, RngCore};

use crate::config::KdfConfig;

use super::error::CryptoError;

pub trait KeyDerivationFunction: Send + Sync {
    fn derive_key(&self, password: &[u8], salt: &[u8]) -> Result<Vec<u8>, CryptoError>;
    fn output_length(&self) -> usize;
    fn salt_length(&self) -> usize;
}

pub trait PasswordHashing: Send + Sync {
    fn hash_password(&self, password: &[u8]) -> Result<String, CryptoError>;
    fn verify_password(&self, password: &[u8], hash: &str) -> Result<bool, CryptoError>;
}

#[derive(Clone)]
pub struct Argon2Kdf {
    argon2: Argon2<'static>,
    output_length: usize,
    salt_length: usize,
}

impl Argon2Kdf {
    pub fn from_config(cfg: &KdfConfig) -> Result<Self, CryptoError> {
        let mut builder = ParamsBuilder::new();
        let memory_kib = cfg
            .memory_mib
            .checked_mul(1024)
            .ok_or_else(|| CryptoError::Derivation("memory parameter overflow".into()))?;
        builder.m_cost(memory_kib);
        builder.t_cost(cfg.iterations);
        builder.p_cost(cfg.parallelism);
        builder.output_len(cfg.output_length as usize);

        let params = builder
            .build()
            .map_err(|err| CryptoError::Derivation(err.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        Ok(Self {
            argon2,
            output_length: cfg.output_length as usize,
            salt_length: cfg.salt_length as usize,
        })
    }
}

impl KeyDerivationFunction for Argon2Kdf {
    fn derive_key(&self, password: &[u8], salt: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if salt.len() < self.salt_length {
            return Err(CryptoError::Derivation(format!(
                "provided salt smaller than configured length ({} < {})",
                salt.len(),
                self.salt_length
            )));
        }
        let mut output = vec![0u8; self.output_length];
        self.argon2
            .hash_password_into(password, salt, &mut output)
            .map_err(|err| CryptoError::Derivation(err.to_string()))?;
        Ok(output)
    }

    fn output_length(&self) -> usize {
        self.output_length
    }

    fn salt_length(&self) -> usize {
        self.salt_length
    }
}

impl PasswordHashing for Argon2Kdf {
    fn hash_password(&self, password: &[u8]) -> Result<String, CryptoError> {
        let mut salt = vec![0u8; self.salt_length];
        let mut rng = OsRng;
        rng.fill_bytes(&mut salt);
        let salt = SaltString::encode_b64(&salt)
            .map_err(|err| CryptoError::Derivation(err.to_string()))?;
        let hash = self.argon2.hash_password(password, &salt)?.to_string();
        Ok(hash)
    }

    fn verify_password(&self, password: &[u8], hash: &str) -> Result<bool, CryptoError> {
        let parsed = PasswordHash::new(hash)?;
        Ok(self.argon2.verify_password(password, &parsed).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_hash_and_verify_roundtrip() {
        let cfg = KdfConfig {
            algorithm: "argon2id".to_string(),
            memory_mib: 8,
            iterations: 2,
            parallelism: 1,
            salt_length: 16,
            output_length: 32,
        };
        let kdf = Argon2Kdf::from_config(&cfg).expect("argon2 config should be valid");
        let hash = kdf
            .hash_password(b"correct horse battery staple")
            .expect("hashing must succeed");
        assert!(kdf
            .verify_password(b"correct horse battery staple", &hash)
            .expect("verification should run"));
        assert!(!kdf
            .verify_password(b"wrong password", &hash)
            .expect("verification should run"));
    }
}
