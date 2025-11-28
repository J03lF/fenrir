pub fn no_ciphers_configured() -> String {
    "no ciphers configured".to_string()
}

pub fn audit_append_failed() -> &'static str {
    "failed to append security audit event"
}

pub fn audit_build_failed() -> &'static str {
    "failed to build security audit event"
}

pub fn missing_token_placeholder() -> &'static str {
    "<none>"
}

pub fn empty_token_placeholder() -> &'static str {
    "<empty>"
}

pub fn fingerprint_display(prefix: &str, total_len: usize) -> String {
    format!("{prefix}…({total_len} bytes)")
}
