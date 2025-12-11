use std::fmt;

pub mod install {
    pub const DISTRIBUTION_LABEL: &str = "Distribution";
    pub const LOCAL_OVERRIDE_LABEL: &str = "Synchronized";
}

pub mod id {
    pub const EMPTY: &str = "module id must not be empty";
    pub const INVALID_CHARS: &str = "module id may only contain [a-z0-9-_]";
}

pub mod version {
    use super::*;

    pub fn invalid(value: &str, err: impl fmt::Display) -> String {
        format!("invalid module version `{value}`: {err}")
    }
}

pub mod validation {
    use super::*;

    pub fn error(message: impl fmt::Display) -> String {
        format!("validation error: {message}")
    }
}

pub mod registry_errors {
    use super::*;

    pub fn unavailable(details: impl fmt::Display) -> String {
        format!("registry unavailable: {details}")
    }

    pub fn not_found(module: &str) -> String {
        format!("module `{module}` was not found")
    }

    pub fn protocol(details: impl fmt::Display) -> String {
        format!("registry protocol error: {details}")
    }
}

pub mod storage_errors {
    use super::*;

    pub fn unavailable(details: impl fmt::Display) -> String {
        format!("storage unavailable: {details}")
    }

    pub fn io(details: impl fmt::Display) -> String {
        format!("io error: {details}")
    }

    pub fn invalid_state(details: impl fmt::Display) -> String {
        format!("invalid state: {details}")
    }
}

pub mod verification_errors {
    use super::*;

    pub fn signature(details: impl fmt::Display) -> String {
        format!("signature verification failed: {details}")
    }

    pub fn checksum(details: impl fmt::Display) -> String {
        format!("checksum verification failed: {details}")
    }

    pub const UNSUPPORTED: &str = "unsupported algorithm";
}

pub mod service_errors {
    use super::*;

    pub fn registry(details: impl fmt::Display) -> String {
        format!("registry error: {details}")
    }

    pub fn storage(details: impl fmt::Display) -> String {
        format!("storage error: {details}")
    }

    pub fn verification(details: impl fmt::Display) -> String {
        format!("verification error: {details}")
    }
}

pub mod runtime_errors {
    use super::*;

    pub fn not_installed(module_id: &str) -> String {
        format!("module `{module_id}` is not installed")
    }

    pub fn already_running(module_id: &str) -> String {
        format!("module `{module_id}` is already running")
    }

    pub fn not_running(module_id: &str) -> String {
        format!("module `{module_id}` is not running")
    }

    pub fn start_failed(module_id: &str, reason: impl fmt::Display) -> String {
        format!("failed to start module `{module_id}`: {reason}")
    }

    pub fn stop_failed(module_id: &str, reason: impl fmt::Display) -> String {
        format!("failed to stop module `{module_id}`: {reason}")
    }

    pub fn port_in_use(port: u16) -> String {
        format!("port {port} is already in use")
    }

    pub fn no_available_ports(start: u16, end: u16) -> String {
        format!("no ports available in range {start}-{end}")
    }

    pub fn io_error(details: impl fmt::Display) -> String {
        format!("io error: {details}")
    }

    pub fn invalid_state(details: impl fmt::Display) -> String {
        format!("invalid state: {details}")
    }

    pub fn env_unavailable(module_id: &str) -> String {
        format!(
            "environment for module `{module_id}` is unavailable; restart the module to capture it"
        )
    }

    pub fn quarantined(module_id: &str, until: &str) -> String {
        format!("module `{module_id}` is quarantined until {until}")
    }
}
