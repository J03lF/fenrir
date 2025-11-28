use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use time::{format_description, OffsetDateTime};

use super::archive::LogKind;
use super::writers::{env_var, prepare_log_file};
use super::{DB_LOG_PATH, DEFAULT_DB_LOG};
use crate::utils::messages;

pub(super) fn prepare_db_log() -> Result<()> {
    let path = env_var("FENRIR_DB_LOG_PATH").unwrap_or_else(|| DEFAULT_DB_LOG.to_string());
    if path.trim().is_empty() {
        return Ok(());
    }
    let current = prepare_log_file(PathBuf::from(path), LogKind::Db)?;
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&current)
        .with_context(|| messages::infra::logging::db::init_failed(current.display()))?;
    DB_LOG_PATH.set(current).ok();
    Ok(())
}

pub fn append_db_log(message: &str) {
    if let Some(path) = DB_LOG_PATH.get() {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            if let Ok(format) =
                format_description::parse("[year]-[month]-[day]T[hour]:[minute]:[second]Z")
            {
                if let Ok(ts) = OffsetDateTime::now_utc().format(&format) {
                    let _ = writeln!(file, "{ts} {message}");
                    return;
                }
            }
            let _ = writeln!(file, "{message}");
        }
    }
}
