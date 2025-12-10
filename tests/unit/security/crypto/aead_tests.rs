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
