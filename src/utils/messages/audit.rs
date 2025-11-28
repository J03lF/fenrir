use std::fmt;
use std::path::Path;

pub mod actors {
    pub const SYSTEM: &str = "system";
    pub const USER: &str = "user";
}

pub mod outcomes {
    pub const SUCCESS: &str = "success";
    pub const FAILURE: &str = "failure";
    pub const DENIED: &str = "denied";
}

pub mod builder {
    pub const MISSING_ACTOR: &str = "audit actor must be set";
    pub const MISSING_ACTION: &str = "action is required";
    pub const MISSING_TARGET: &str = "target is required";
}

pub mod errors {
    pub const LOG_LOCKED: &str = "audit log locked";
}

pub mod persistence {
    use super::*;

    pub const HISTORY_DIR_CREATE_FAILED: &str = "audit history directory creation failed";
    pub const HISTORY_PERSIST_FAILED: &str = "audit history persist failed";
    pub const HISTORY_SERIALIZATION_FAILED: &str = "audit history serialization failed";
    pub const HISTORY_PARSE_FAILED: &str = "audit history parse failed";
    pub const HISTORY_READ_FAILED: &str = "audit history read failed";

    pub fn path_not_writable(path: &Path, err: impl fmt::Display) -> String {
        format!(
            "audit persistence path '{}' not writable: {err}",
            path.display()
        )
    }
}
