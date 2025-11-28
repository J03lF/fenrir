use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use time::{format_description, OffsetDateTime};

use super::{db_log_file_path, log_file_path, ARCHIVE_DIR, MAX_ARCHIVES};
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
    let prefix = kind.prefix();
    let archive_dir = archive_dir_for(&current);
    let entries = fs::read_dir(&archive_dir).ok()?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with(prefix) && name.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths.into_iter().last()
}

pub(super) fn rotate_if_exists(current: &Path, prefix: &str) -> Result<()> {
    if !current.exists() {
        return Ok(());
    }
    let archive_dir = archive_dir_for(current);
    fs::create_dir_all(&archive_dir).with_context(|| {
        messages::infra::logging::archive::create_dir_failed(archive_dir.display())
    })?;
    let ts_format = format_description::parse("[year][month][day]_[hour][minute][second]")?;
    let timestamp = OffsetDateTime::now_utc().format(&ts_format)?;
    let archive_name = format!("{prefix}-{timestamp}.log");
    let archive_path = archive_dir.join(archive_name);
    fs::rename(current, &archive_path).with_context(|| {
        messages::infra::logging::archive::rotate_move_failed(
            current.display(),
            archive_path.display(),
        )
    })?;
    prune_archives(&archive_dir, prefix, MAX_ARCHIVES)?;
    Ok(())
}

pub(super) fn prune_archives(dir: &Path, prefix: &str, keep: usize) -> Result<()> {
    let mut archives: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with(prefix) && name.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    archives.sort();
    while archives.len() > keep {
        if let Some(oldest) = archives.first() {
            fs::remove_file(oldest)?;
        }
        archives.remove(0);
    }
    Ok(())
}

pub(super) fn archive_dir_for(current: &Path) -> PathBuf {
    current
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(ARCHIVE_DIR)
}
