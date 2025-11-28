pub mod warnings {
    pub const DESTRUCTIVE_FORCE_WARNING: &str =
        "Destructive commands require '--force' at the end of the line.";
}

pub mod errors {
    pub const SERVICE_DISABLED: &str = "db-shell service is disabled";
}
