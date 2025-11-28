pub mod audit;
pub mod clear;
pub mod db_shell;
pub mod exit;
pub mod help;
pub mod log;
pub mod modules;
mod registry;
pub mod services;
pub mod show;
pub mod user;

pub use registry::{build_registry, register_builtins};
