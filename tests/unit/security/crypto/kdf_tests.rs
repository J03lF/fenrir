use super::*;

#[test]
fn argon2_hash_and_verify_roundtrip() {
    let cfg = KdfConfig {
        algorithm: "argon2id".to_string(),
        version: 1,
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
