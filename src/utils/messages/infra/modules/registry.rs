use std::fmt;

pub const URL_EMPTY: &str = "modules.registry.url must not be empty";
pub const AUTH_TOKEN_INVALID_CHARS: &str =
    "modules.registry.auth_token contains invalid characters";
pub const CLIENT_CERT_KEY_MISMATCH: &str =
    "modules.registry.tls.client_cert_path and client_key_path must be provided together";
pub const CHECKSUM_MISMATCH_RETRY: &str = "artifact checksum mismatch, attempting manifest refresh";
pub const CHECKSUM_MISMATCH_RETRY_ONCE: &str = "retrying download once after checksum mismatch";
pub const CHECKSUM_MISMATCH_MANIFEST_REFRESH: &str = "retrying download with refreshed manifest";
pub const CHECKSUM_REFRESH_FAILED: &str = "failed to refresh manifest after checksum mismatch";
pub const CHECKSUM_MISMATCH_FATAL: &str = "Downloaded artifact checksum mismatch";
pub const OFFLINE_ROOT_MISSING: &str = "offline registry root missing, skipping";
pub const LOCAL_MANIFEST_INVALID: &str = "invalid local module manifest";
pub const LOCAL_MANIFEST_LOAD_FAILED: &str = "failed to load local module manifest";
pub const COMPOSITE_NO_SOURCES: &str = "composite registry requires at least one source";
pub const LOCAL_VERSION_SKIP: &str = "skipping local module version";
pub const LOCAL_MANIFEST_NO_VALID_VERSIONS: &str = "no valid versions";
pub const LOCAL_MANIFEST_NO_VERSIONS: &str = "no versions";
pub const LOCAL_MANIFEST_CHECKSUM_MISSING: &str = "checksum missing";

pub fn ca_cert_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("Failed to read {}: {err}", path)
}

pub fn ca_cert_invalid_pem(err: impl fmt::Display) -> String {
    format!("modules.registry.tls.ca_cert_path is not valid PEM: {err}")
}

pub fn client_cert_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("Failed to read client certificate {}: {err}", path)
}

pub fn client_key_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("Failed to read client key {}: {err}", path)
}

pub fn client_identity_build_failed(err: impl fmt::Display) -> String {
    format!(
        "modules.registry.tls.client_cert_path/client_key_path could not be combined into a valid identity: {err}"
    )
}

pub fn base_url_invalid(err: impl fmt::Display) -> String {
    format!("Invalid registry base URL: {err}")
}

pub fn download_url_resolve_failed(url: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("Failed to resolve download URL '{url}': {err}")
}

pub fn artifact_download_failed(err: impl fmt::Display) -> String {
    format!("Failed to download artifact: {err}")
}

pub fn download_status_failed(status: impl fmt::Display) -> String {
    format!("Download failed with status: {status}")
}

pub fn download_chunk_failed(err: impl fmt::Display) -> String {
    format!("Failed to read chunk: {err}")
}

pub fn checksum_invalid_encoding(err: impl fmt::Display) -> String {
    format!("Invalid checksum encoding: {err}")
}

pub fn signature_invalid_encoding(err: impl fmt::Display) -> String {
    format!("Invalid signature encoding: {err}")
}

pub fn registry_query_failed(err: impl fmt::Display) -> String {
    format!("Failed to query registry: {err}")
}

pub fn registry_status_failed(status: impl fmt::Display) -> String {
    format!("Registry returned status: {status}")
}

pub fn registry_response_parse_failed(err: impl fmt::Display) -> String {
    format!("Failed to parse registry response: {err}")
}

pub fn module_fetch_failed(err: impl fmt::Display) -> String {
    format!("Failed to fetch module: {err}")
}

pub fn module_payload_parse_failed(err: impl fmt::Display) -> String {
    format!("Failed to parse module payload: {err}")
}

pub fn module_no_versions(module: impl fmt::Display) -> String {
    format!("Module {module} has no published versions")
}

pub fn version_invalid(err: impl fmt::Display) -> String {
    format!("Invalid version string: {err}")
}

pub fn compatibility_query_failed(err: impl fmt::Display) -> String {
    format!("Failed to query compatibility tree: {err}")
}

pub fn compatibility_status_failed(status: impl fmt::Display) -> String {
    format!("Compatibility API returned status: {status}")
}

pub fn compatibility_response_invalid(err: impl fmt::Display) -> String {
    format!("Invalid compatibility response: {err}")
}

pub fn compatibility_tree_invalid(err: impl fmt::Display) -> String {
    format!("Invalid compatibility tree format: {err}")
}

pub fn module_id_invalid(err: impl fmt::Display) -> String {
    format!("invalid module id: {err}")
}

pub fn fenrir_version_invalid(err: impl fmt::Display) -> String {
    format!("invalid Fenrir version: {err}")
}

pub fn artifact_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("failed to read artifact {}: {err}", path)
}

pub fn artifact_checksum_invalid_hex(err: impl fmt::Display) -> String {
    format!("invalid checksum hex: {err}")
}

pub fn artifact_checksum_mismatch(path: impl fmt::Display) -> String {
    format!("checksum mismatch for artifact {}", path)
}

pub fn artifact_signature_invalid_encoding(err: impl fmt::Display) -> String {
    format!("invalid signature encoding: {err}")
}

pub fn local_manifest_read_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot read {}: {err}", path)
}

pub fn local_manifest_parse_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
    format!("cannot parse {}: {err}", path)
}

pub fn local_manifest_invalid_field(field: impl fmt::Display) -> String {
    format!("invalid manifest: {field}")
}

pub fn local_manifest_artifact_missing(path: impl fmt::Display) -> String {
    format!("missing artifact at {}", path)
}

pub fn auth_token_env_missing(var: impl fmt::Display) -> String {
    format!(
        "Environment variable {} referenced in modules.registry.auth_token is not set",
        var
    )
}

pub fn resolved_empty(field: impl fmt::Display) -> String {
    format!("{} must not resolve to an empty value", field)
}
