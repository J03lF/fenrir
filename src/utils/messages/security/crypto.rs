pub fn unsupported_cipher_algorithm(value: &str) -> String {
    format!("unsupported cipher algorithm: {value}")
}

pub fn invalid_key_length() -> &'static str {
    "invalid key length for selected cipher"
}

pub fn invalid_nonce_length() -> &'static str {
    "invalid nonce length for selected cipher"
}

pub fn derivation_failed(reason: &str) -> String {
    format!("key derivation failed: {reason}")
}

pub fn password_hash_failed(reason: &str) -> String {
    format!("password hashing failed: {reason}")
}

pub fn encryption_failed() -> &'static str {
    "encryption failed"
}

pub fn decryption_failed() -> &'static str {
    "decryption failed"
}

pub fn randomness_unavailable(reason: &str) -> String {
    format!("randomness source unavailable: {reason}")
}

pub fn memory_parameter_overflow() -> String {
    "memory parameter overflow".to_string()
}

pub fn salt_length_too_short(provided: usize, required: usize) -> String {
    format!("provided salt smaller than configured length ({provided} < {required})")
}
