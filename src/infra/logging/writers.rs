use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

use super::archive::{rotate_if_exists, LogKind};
use super::DEFAULT_APP_LOG;
use crate::utils::messages;

pub(super) fn build_app_writer() -> Result<Option<(NonBlocking, WorkerGuard, PathBuf)>> {
    let path = env_var("FENRIR_LOG_PATH").unwrap_or_else(|| DEFAULT_APP_LOG.to_string());
    if path.trim().is_empty() {
        return Ok(None);
    }
    let current = prepare_log_file(PathBuf::from(path), LogKind::App)?;
    let parent = current
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = current
        .file_name()
        .ok_or_else(|| anyhow!(messages::infra::logging::writers::INVALID_LOG_PATH))?
        .to_owned();
    let appender = tracing_appender::rolling::never(parent, file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    Ok(Some((non_blocking, guard, current)))
}

pub(super) fn prepare_log_file(path: PathBuf, kind: LogKind) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            messages::infra::logging::writers::create_dir_failed(parent.display())
        })?;
    }
    rotate_if_exists(&path, kind.prefix())?;
    Ok(path)
}

pub(super) fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
