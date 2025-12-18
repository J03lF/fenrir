use std::fmt;

pub mod errors {
    use super::fmt;

    pub const DEV_SOURCES_DISABLED: &str =
        "modules.dev_sources.base_path must be configured to scaffold modules";

    pub fn module_exists(path: impl fmt::Display) -> String {
        format!("module directory {path} already exists")
    }

    pub fn directory_create_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to create {path}: {err}")
    }

    pub fn write_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to write {path}: {err}")
    }

    pub fn metadata_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to inspect {path}: {err}")
    }

    pub fn permission_failed(path: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("failed to update permissions for {path}: {err}")
    }
}
