use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),
    #[error("invalid configuration: {0}")]
    Invalid(&'static str),
    #[error("invalid configuration: {0}")]
    InvalidMessage(String),
    #[error("missing environment variable {var} for {key}")]
    MissingEnv { key: &'static str, var: String },
    #[error("config profile '{value}' contains invalid characters (allowed: a-z, 0-9, '-', '_')")]
    InvalidProfile { value: String },
    #[error("config file not found: {path}")]
    MissingConfigFile { path: String },
}
