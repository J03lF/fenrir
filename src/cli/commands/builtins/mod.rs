pub mod audit;
pub mod backup;
pub mod clear;
pub mod db_shell;
pub mod db_runtime;  // Used by log/status/start/stop/restart db
pub mod db_schema;   // Used by export/import schema
pub mod exit;
pub mod export;
pub mod help;
pub mod import;
pub mod jobs;
pub mod log;
pub mod modules;
mod registry;
pub mod restore;
pub mod services;
pub mod show;
pub mod status;
pub mod user;

pub use registry::{build_registry, register_builtins};
