use std::fmt;

pub mod logs {
    pub const DECLARED_SERVICES_REFRESH_FAILED: &str =
        "failed to refresh declared services for already installed module";
    pub const AUTO_START_FAILED: &str = "failed to auto-start module";
    pub const UPDATE_FAILED: &str = "failed to update module";
}

pub mod notes {
    use super::fmt;

    pub const INSTALLED: &str = "installed";

    pub fn start_failed(err: impl fmt::Display) -> String {
        format!("start failed: {err}")
    }
}

pub mod errors {
    use super::fmt;

    pub fn module_not_installed(id: impl fmt::Display) -> String {
        format!("Module {id} is not installed")
    }

    pub fn module_missing(id: impl fmt::Display) -> String {
        format!("Module {id} is not installed")
    }

    pub fn already_on_distribution(id: impl fmt::Display) -> String {
        format!("Module {id} already uses the distribution")
    }

    pub fn not_part_of_distribution(
        id: impl fmt::Display,
        fenrir_version: impl fmt::Display,
    ) -> String {
        format!("Module {id} is not part of the distribution for Fenrir {fenrir_version}")
    }

    pub fn incompatible_version(
        id: impl fmt::Display,
        module_version: impl fmt::Display,
        fenrir_version: impl fmt::Display,
    ) -> String {
        format!("Module {id} v{module_version} is not compatible with Fenrir {fenrir_version}")
    }
}
