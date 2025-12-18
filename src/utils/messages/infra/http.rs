pub struct ProblemText {
    pub code: &'static str,
    pub message: String,
}

impl ProblemText {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub mod problems {
    use super::ProblemText;
    use std::fmt;

    pub fn module_service_unavailable() -> ProblemText {
        ProblemText::new("module_service_unavailable", "module service unavailable")
    }

    pub fn token_exchange_unavailable() -> ProblemText {
        ProblemText::new(
            "token_exchange_unavailable",
            "token exchange service unavailable",
        )
    }

    pub fn token_exchange_rate_limited() -> ProblemText {
        ProblemText::new(
            "token_exchange_rate_limited",
            "token exchange rate limit exceeded",
        )
    }

    pub fn identity_service_unavailable() -> ProblemText {
        ProblemText::new(
            "identity_service_unavailable",
            "identity service unavailable",
        )
    }

    pub fn invalid_role(value: &str) -> ProblemText {
        ProblemText::new("invalid_role", format!("unknown role '{value}'"))
    }

    pub fn role_insufficient_viewer() -> ProblemText {
        ProblemText::new("role_insufficient", "role viewer required at minimum")
    }

    pub fn role_insufficient_admin() -> ProblemText {
        ProblemText::new("role_insufficient", "action requires role admin")
    }

    pub fn role_insufficient(required: &str, actual: &str) -> ProblemText {
        ProblemText::new(
            "role_insufficient",
            format!("action requires role {required}, current role {actual} is insufficient"),
        )
    }

    pub fn audit_unavailable(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "audit_unavailable",
            format!("audit store unavailable: {err}"),
        )
    }

    pub fn invalid_level_empty() -> ProblemText {
        ProblemText::new("invalid_level", "log level must not be empty")
    }

    pub fn logging_reload_unavailable() -> ProblemText {
        ProblemText::new(
            "logging_reload_unavailable",
            "no logging reload handle registered",
        )
    }

    pub fn logging_reload_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "logging_reload_failed",
            format!("failed to set log level: {err}"),
        )
    }

    pub fn identity_list_failed() -> ProblemText {
        ProblemText::new("identity_list_failed", "failed to load identity users")
    }

    pub fn identity_task_failed() -> ProblemText {
        ProblemText::new("identity_task_failed", "failed to load identity users")
    }

    pub fn identity_issue_failed() -> ProblemText {
        ProblemText::new("identity_issue_failed", "failed to issue token")
    }

    pub fn identity_issue_task_failed() -> ProblemText {
        ProblemText::new("identity_task_failed", "failed to issue token")
    }

    pub fn service_unknown(id: &str) -> ProblemText {
        ProblemText::new(
            "unknown_service",
            format!("service `{id}` is not registered."),
        )
    }

    pub fn service_not_controllable(id: &str) -> ProblemText {
        ProblemText::new(
            "not_controllable",
            format!("service `{id}` cannot be controlled over HTTP."),
        )
    }

    pub fn service_force_required(id: &str) -> ProblemText {
        ProblemText::new(
            "force_required",
            format!(
                "service `{id}` is marked as critical. Please confirm the action with force=true."
            ),
        )
    }

    pub fn service_core_locked(id: &str) -> ProblemText {
        ProblemText::new(
            "core_locked",
            format!(
                "service `{id}` belongs to the core platform and cannot be stopped or restarted."
            ),
        )
    }

    pub fn service_operation_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new("operation_failed", format!("action failed: {err}"))
    }

    pub fn unauthorized() -> ProblemText {
        ProblemText::new("unauthorized", "authorization required")
    }

    pub fn forbidden() -> ProblemText {
        ProblemText::new("forbidden", "access denied")
    }

    pub fn module_token_service_only() -> ProblemText {
        ProblemText::new(
            "module_token_service_only",
            "token exchange is limited to module service tokens",
        )
    }

    pub fn module_token_invalid_service(id: &str) -> ProblemText {
        ProblemText::new(
            "module_token_invalid_service",
            format!("'{id}' is not a managed module service"),
        )
    }

    pub fn module_token_scope_invalid(scope: &str) -> ProblemText {
        ProblemText::new(
            "module_token_scope_invalid",
            format!("scope '{scope}' is not allowed for module tokens"),
        )
    }

    pub fn module_token_issue_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "module_token_issue_failed",
            format!("failed to issue module token: {err}"),
        )
    }

    pub fn gateway_invalid_service(value: &str) -> ProblemText {
        ProblemText::new(
            "gateway_invalid_service",
            format!("service identifier '{value}' is invalid"),
        )
    }

    pub fn gateway_unknown_service(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_unknown_service",
            format!("service '{id}' is not registered for ingress"),
        )
    }

    pub fn static_module_invalid(id: &str) -> ProblemText {
        ProblemText::new(
            "static_module_invalid",
            format!("module identifier '{id}' is invalid"),
        )
    }

    pub fn static_module_not_running(id: &str) -> ProblemText {
        ProblemText::new(
            "static_module_not_running",
            format!("static module '{id}' is not running"),
        )
    }

    pub fn static_module_not_static(id: &str) -> ProblemText {
        ProblemText::new(
            "static_module_not_static",
            format!("module '{id}' is not a static site"),
        )
    }

    pub fn static_module_unreachable(id: &str) -> ProblemText {
        ProblemText::new(
            "static_module_unreachable",
            format!("static module '{id}' is temporarily unavailable"),
        )
    }

    pub fn gateway_security_unavailable() -> ProblemText {
        ProblemText::new(
            "gateway_security_unavailable",
            "security manager unavailable for service gateway",
        )
    }

    pub fn gateway_security_missing(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_security_missing",
            format!("service '{id}' is missing security metadata"),
        )
    }

    pub fn gateway_token_missing() -> ProblemText {
        ProblemText::new(
            "gateway_token_missing",
            "service token missing from Authorization header",
        )
    }

    pub fn gateway_token_invalid() -> ProblemText {
        ProblemText::new(
            "gateway_token_invalid",
            "service token is malformed or unsupported",
        )
    }

    pub fn gateway_token_expired() -> ProblemText {
        ProblemText::new(
            "gateway_token_expired",
            "service token is expired, revoked, or idle timed out",
        )
    }

    pub fn gateway_security_failure(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "gateway_security_failure",
            format!("security subsystem rejected token: {err}"),
        )
    }

    pub fn gateway_access_denied(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_access_denied",
            format!("service '{id}' denied the provided token"),
        )
    }

    pub fn gateway_service_unavailable(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_service_unavailable",
            format!("service '{id}' is not currently running"),
        )
    }

    pub fn gateway_service_failure(id: &str, err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "gateway_service_failure",
            format!("service '{id}' cannot be reached: {err}"),
        )
    }

    pub fn gateway_rate_limited(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_rate_limited",
            format!("service '{id}' exceeded the rate limit"),
        )
    }

    pub fn gateway_protocol_unsupported(id: &str, protocol: &str) -> ProblemText {
        ProblemText::new(
            "gateway_protocol_unsupported",
            format!("service '{id}' does not expose {protocol} ingress"),
        )
    }

    pub fn gateway_upstream_unreachable(id: &str) -> ProblemText {
        ProblemText::new(
            "gateway_upstream_unreachable",
            format!("service '{id}' is unreachable"),
        )
    }

    pub fn gateway_upstream_read_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "gateway_upstream_read_failed",
            format!("failed to read upstream response: {err}"),
        )
    }

    pub fn gateway_upstream_conversion_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "gateway_upstream_conversion_failed",
            format!("failed to map upstream response: {err}"),
        )
    }

    pub fn gateway_method_not_allowed(method: &str) -> ProblemText {
        ProblemText::new(
            "gateway_method_not_allowed",
            format!("HTTP method '{method}' is not supported"),
        )
    }

    pub fn gateway_body_read_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "gateway_body_read_failed",
            format!("failed to read request body: {err}"),
        )
    }
}

pub mod tls {
    use std::fmt;

    pub const CERT_PATH_REQUIRED: &str =
        "server.http.tls.cert_path must be set when TLS is enabled";
    pub const KEY_PATH_REQUIRED: &str = "server.http.tls.key_path must be set when TLS is enabled";
    pub const RUNTIME_LOCK_POISONED: &str = "tls runtime lock poisoned";
    pub const WATCHER_LOCK_POISONED: &str = "tls watcher lock poisoned";
    pub const RELOAD_LOCK_POISONED: &str = "tls reload lock poisoned";
    pub const FILE_WATCHER_CREATE_FAILED: &str = "failed to create TLS file watcher";
    pub const CERT_WATCH_FAILED: &str = "unable to watch TLS certificate";
    pub const KEY_WATCH_FAILED: &str = "unable to watch TLS key";
    pub const RELOAD_FAILED: &str = "failed to reload TLS certificates";
    pub const RELOAD_SUCCESS_FILESYSTEM: &str = "TLS certificates reloaded (filesystem event)";
    pub const WATCH_ERROR: &str = "error while watching TLS artifacts";
    pub const REQUEST_FAILED: &str = "TLS request handling failed";
    pub const INTERNAL_SERVER_ERROR_BODY: &str = "internal server error";
    pub const HOOKS_REGISTER_POISONED: &str =
        "TLS reload hooks poisoned; unable to register new hook";
    pub const HOOKS_NOTIFY_POISONED: &str = "TLS reload hooks poisoned; skipping notifications";
    pub const HOOK_PANICKED: &str = "TLS reload hook panicked";
    pub const CERT_PARSE_FAILED: &str = "failed to parse certificate";
    pub const CERT_CHAIN_EMPTY: &str = "no certificate chains found";
    pub const PKCS8_PARSE_FAILED: &str = "failed to parse private key (PKCS8)";
    pub const RSA_PARSE_FAILED: &str = "failed to parse private key (RSA)";
    pub const PRIVATE_KEY_MISSING: &str = "no private key found";

    pub fn cert_not_found(path: impl fmt::Display) -> String {
        format!("TLS certificate '{path}' was not found")
    }

    pub fn key_not_found(path: impl fmt::Display) -> String {
        format!("TLS key '{path}' was not found")
    }

    pub fn cert_read_failed(path: impl fmt::Display) -> String {
        format!("failed to read certificate '{path}'")
    }

    pub fn key_read_failed(path: impl fmt::Display) -> String {
        format!("failed to read key '{path}'")
    }

    pub fn cipher_suite_unknown(name: &str) -> String {
        format!("unknown cipher suite: {name}")
    }
}
