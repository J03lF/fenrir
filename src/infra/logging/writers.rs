use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};

use super::archive::rotate_if_exists;
use super::retention::{LogRetention, RotatingFile};
use super::DEFAULT_APP_LOG;
use crate::utils::messages;

pub(super) fn build_app_writer(
    policy: &LogRetention,
) -> Result<Option<(NonBlocking, WorkerGuard, PathBuf)>> {
    let path = env_var("FENRIR_LOG_PATH").unwrap_or_else(|| DEFAULT_APP_LOG.to_string());
    if path.trim().is_empty() {
        return Ok(None);
    }
    let current = prepare_log_file(PathBuf::from(path), policy)?;
    if current.file_name().is_none() {
        return Err(anyhow!(messages::infra::logging::writers::INVALID_LOG_PATH));
    }
    // Daily or at the size cap, 0600, archives pruned (W9A-13).
    let appender = RotatingFile::open(current.clone(), *policy)
        .with_context(|| messages::infra::logging::writers::open_failed(current.display()))?;
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    Ok(Some((non_blocking, guard, current)))
}

pub(super) fn prepare_log_file(path: PathBuf, policy: &LogRetention) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            messages::infra::logging::writers::create_dir_failed(parent.display())
        })?;
    }
    rotate_if_exists(&path, policy)?;
    Ok(path)
}

pub(super) fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
