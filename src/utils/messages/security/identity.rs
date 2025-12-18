// Common validation + auth messages
pub fn unsupported_signing_algorithm() -> &'static str {
    "unsupported signing algorithm"
}

pub fn unknown_signing_key() -> &'static str {
    "unknown signing key"
}

pub fn signature_verification_failed() -> &'static str {
    "signature verification failed"
}

pub fn invalid_token_claims_payload() -> &'static str {
    "invalid token claims payload"
}

pub fn issuer_mismatch() -> &'static str {
    "issuer mismatch"
}

pub fn audience_mismatch() -> &'static str {
    "audience mismatch"
}

pub fn environment_mismatch() -> &'static str {
    "environment mismatch"
}

pub fn token_expired() -> &'static str {
    "token expired"
}

pub fn unknown_role_claim(value: &str) -> String {
    format!("unknown role claim '{value}'")
}

pub fn invalid_issued_at_timestamp() -> &'static str {
    "invalid issued-at timestamp"
}

pub fn invalid_expiry_timestamp() -> &'static str {
    "invalid expiry timestamp"
}

pub fn token_timestamp_out_of_range(kind: &str) -> String {
    format!("token {kind} timestamp out of range")
}

pub fn password_auth_not_supported_embedded() -> &'static str {
    "password authentication not supported for embedded identity provider"
}

pub fn user_not_found(user_id: &str) -> String {
    format!("user '{}' not found", user_id)
}

pub fn invalid_credentials() -> &'static str {
    "invalid credentials"
}

// Audit + logging
pub fn audit_append_failed() -> &'static str {
    "failed to append identity audit event"
}

pub fn audit_build_failed() -> &'static str {
    "failed to build identity audit event"
}

pub fn skipping_identity_user_invalid_role() -> &'static str {
    "skipping identity user with invalid role"
}

pub fn failed_refresh_jwks_use_cached() -> &'static str {
    "failed to refresh identity JWKS; using cached keys"
}

pub fn skipping_unsupported_jwks_key_type() -> &'static str {
    "skipping unsupported JWKS key type"
}

pub fn skipping_jwks_key_unexpected_length() -> &'static str {
    "skipping JWKS key with unexpected length"
}

pub fn identity_jwks_updated() -> &'static str {
    "identity JWKS updated"
}

// Provider + TLS helpers
pub fn derive_jwks_url_failed(err: &str) -> String {
    format!("failed to derive JWKS url: {err}")
}

pub fn read_tls_ca_failed(path: &str, err: &std::io::Error) -> String {
    format!(
        "failed to read identity TLS CA certificate '{}': {err}",
        path
    )
}

pub fn tls_ca_invalid_pem(err: &str) -> String {
    format!("identity TLS CA certificate is not valid PEM: {err}")
}

pub fn read_client_cert_failed(path: &str, err: &std::io::Error) -> String {
    format!(
        "failed to read identity client certificate '{}': {err}",
        path
    )
}

pub fn read_client_key_failed(path: &str, err: &std::io::Error) -> String {
    format!("failed to read identity client key '{}': {err}", path)
}

pub fn client_identity_invalid_pem(err: &str) -> String {
    format!("identity client certificate/key is not valid PEM: {err}")
}

pub fn identity_client_tls_incomplete() -> &'static str {
    "identity client certificate configuration is incomplete"
}

pub fn invalid_identity_url(value: &str, err: &str) -> String {
    format!("invalid identity url '{value}': {err}")
}

// External provider + HTTP client
pub fn invalid_identity_response(err: &str) -> String {
    format!("invalid identity response: {err}")
}

pub fn invalid_token_claims(err: &str) -> String {
    format!("invalid token claims: {err}")
}

pub fn invalid_identity_users_response(err: &str) -> String {
    format!("invalid identity users response: {err}")
}

pub fn invalid_identity_login_response(err: &str) -> String {
    format!("invalid identity login response: {err}")
}

pub fn invalid_claims(err: &str) -> String {
    format!("invalid claims: {err}")
}

pub fn invalid_jwks_document(err: &str) -> String {
    format!("invalid JWKS document: {err}")
}

pub fn identity_jwks_no_keys() -> &'static str {
    "identity JWKS document contained no keys"
}

pub fn invalid_jwks_key_encoding(err: &str) -> String {
    format!("invalid JWKS key encoding: {err}")
}

pub fn failed_parse_jwks_public_key() -> &'static str {
    "failed to parse JWKS public key"
}

pub fn no_supported_jwks_keys() -> &'static str {
    "no supported keys found in identity JWKS"
}

pub fn failed_build_identity_client(err: &str) -> String {
    format!("failed to build identity client: {err}")
}

pub fn identity_request_failed(err: &str) -> String {
    format!("identity server request failed: {err}")
}

pub fn identity_server_status(status: &str) -> String {
    format!("identity server returned status {status}")
}

pub fn invalid_identity_path(err: &str) -> String {
    format!("invalid identity path: {err}")
}

// Store & persistence
pub fn stored_key_material_invalid() -> &'static str {
    "stored key material invalid"
}

pub fn stored_public_key_invalid() -> &'static str {
    "stored public key invalid"
}

pub fn unsupported_identity_store_version(version: u32) -> String {
    format!("unsupported identity store version {version}")
}

pub fn identity_store_environment_mismatch(expected: &str, found: &str) -> String {
    format!("identity store environment mismatch (expected {expected}, found {found})")
}

pub fn identity_store_instance_mismatch(expected: &str, found: &str) -> String {
    format!("identity store instance mismatch (expected {expected}, found {found})")
}

pub fn unable_to_initialize_secret_key() -> &'static str {
    "unable to initialize secret key"
}

pub fn unknown_role_in_identity_store(value: &str) -> String {
    format!("unknown role '{value}' in identity store")
}

pub fn invalid_base64(err: &str) -> String {
    format!("invalid base64: {err}")
}

pub fn unexpected_key_length_in_store() -> &'static str {
    "unexpected key length in identity store"
}

// JWT parsing helpers
pub fn invalid_token_missing_header() -> &'static str {
    "invalid token: missing header"
}

pub fn invalid_token_missing_claims() -> &'static str {
    "invalid token: missing claims"
}

pub fn invalid_token_missing_signature() -> &'static str {
    "invalid token: missing signature"
}

pub fn invalid_token_too_many_segments() -> &'static str {
    "invalid token: too many segments"
}

pub fn invalid_token_header_encoding() -> &'static str {
    "invalid token header encoding"
}

pub fn invalid_token_header_payload() -> &'static str {
    "invalid token header payload"
}

pub fn invalid_token_claims_encoding() -> &'static str {
    "invalid token claims encoding"
}

pub fn invalid_signature_encoding() -> &'static str {
    "invalid signature encoding"
}

pub fn invalid_signature_length() -> &'static str {
    "invalid signature length"
}

// High-level IdentityError formatting helpers
pub fn store_io_error(err: &std::io::Error) -> String {
    format!("identity store io error: {err}")
}

pub fn store_serialization_error(err: &serde_json::Error) -> String {
    format!("identity store serialization error: {err}")
}

pub fn state_poisoned() -> &'static str {
    "identity state poisoned"
}

pub fn data_invalid(reason: &str) -> String {
    format!("identity data invalid: {reason}")
}

pub fn authorization_failed(reason: &str) -> String {
    format!("identity authorization failed: {reason}")
}
