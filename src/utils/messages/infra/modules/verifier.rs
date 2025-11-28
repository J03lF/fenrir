use std::fmt;

pub const NO_KEYS_LOADED: &str = "no verifying keys loaded but signatures are required";
pub const ALLOWLIST_MISSING_KEYS: &str =
    "allowlisted signers missing from keyring - signature verification will be skipped for them";
pub const SIGNATURE_REQUIRED_EMPTY: &str = "signature is required but empty";
pub const SIGNATURE_LENGTH_EXACT: &str = "signature must be 64 bytes";

pub fn keyring_invalid_json(err: impl fmt::Display) -> String {
    format!("invalid keyring json: {err}")
}

pub fn public_key_invalid_base64(key: &str, err: impl fmt::Display) -> String {
    format!("public key {key} invalid base64: {err}")
}

pub fn public_key_length_invalid(key: &str) -> String {
    format!("public key {key} must be 32 bytes")
}

pub fn public_key_invalid(key: &str, err: impl fmt::Display) -> String {
    format!("public key {key} invalid: {err}")
}

pub fn signer_not_in_allowlist(signer: &str) -> String {
    format!("signer {signer} is not in allowlist")
}

pub fn checksum_mismatch(module: impl fmt::Display) -> String {
    format!("checksum mismatch for module {}", module)
}

pub fn missing_verifier_for_signer(signer: &str) -> String {
    format!("missing verifying key for signer {signer}")
}

pub fn signature_without_key(signer: &str) -> String {
    format!("signature provided but no verifying key for signer {signer}")
}

pub fn signature_length_invalid(len: usize) -> String {
    format!("signature must be 64 bytes, got {} bytes", len)
}
