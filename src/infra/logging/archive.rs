use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use time::OffsetDateTime;

use super::retention::{archive_by_rename, archive_dir_for, is_archive_of, log_stem, LogRetention};
use super::{db_log_file_path, log_file_path};
use crate::utils::messages;

#[derive(Clone, Copy)]
pub enum LogKind {
    App,
    Db,
}

impl LogKind {
    pub fn prefix(&self) -> &'static str {
        match self {
            LogKind::App => "app",
            LogKind::Db => "db",
        }
    }
}

pub fn latest_archive(kind: LogKind) -> Option<PathBuf> {
    let current = match kind {
        LogKind::App => log_file_path()?,
        LogKind::Db => db_log_file_path()?,
    };
    let stem = log_stem(&current);
    let archive_dir = archive_dir_for(&current);
    let entries = fs::read_dir(&archive_dir).ok()?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| is_archive_of(name, &stem))
        })
        .collect();
    paths.sort();
    paths.into_iter().last()
}

/// Moves the log of the previous run into the archive and prunes it
/// (`[telemetry.logs]`).
pub(super) fn rotate_if_exists(current: &Path, policy: &LogRetention) -> Result<()> {
    if !current.exists() {
        return Ok(());
    }
    let archive_dir = archive_dir_for(current);
    fs::create_dir_all(&archive_dir).with_context(|| {
        messages::infra::logging::archive::create_dir_failed(archive_dir.display())
    })?;
    archive_by_rename(current, policy, OffsetDateTime::now_utc()).with_context(|| {
        messages::infra::logging::archive::rotate_move_failed(
            current.display(),
            archive_dir.display(),
        )
    })?;
    Ok(())
}
