pub trait KeyDerivationFunction {
    fn derive_key(&self, password: &[u8], salt: &[u8]) -> Vec<u8>;
}

pub trait AeadCipher {
    fn encrypt(&self, key: &[u8], nonce: &[u8], plaintext: &[u8], aad: &[u8]) -> Vec<u8>;
    fn decrypt(
        &self,
        key: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, DecryptError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DecryptError {
    #[error("decryption failed")]
    Failed,
}
