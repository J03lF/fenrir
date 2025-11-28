mod archive;
mod db;
mod handle;
mod init;
mod writers;

use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;

pub use archive::{latest_archive, LogKind};
pub use db::append_db_log;
pub use handle::ReloadHandle;
pub use init::init_tracing;

const DEFAULT_APP_LOG: &str = "logs/app.log";
const DEFAULT_DB_LOG: &str = "logs/db.log";
const ARCHIVE_DIR: &str = "archive";
const MAX_ARCHIVES: usize = 3;

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static DB_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn reload(handle: &ReloadHandle, level: &str) -> Result<()> {
    handle.reload_level(level)
}

pub fn log_file_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}

pub fn db_log_file_path() -> Option<PathBuf> {
    DB_LOG_PATH.get().cloned()
}
