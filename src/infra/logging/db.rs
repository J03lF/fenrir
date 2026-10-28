use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use time::{format_description, OffsetDateTime};

use super::retention::{create_private, LogRetention, RotatingFile};
use super::writers::{env_var, prepare_log_file};
use super::{DB_LOG_PATH, DEFAULT_DB_LOG};
use crate::utils::messages;

/// The open DB log; rotated like the app log (W9A-13).
static DB_LOG: Mutex<Option<RotatingFile>> = Mutex::new(None);

pub(super) fn prepare_db_log(policy: &LogRetention) -> Result<()> {
    let path = env_var("FENRIR_DB_LOG_PATH").unwrap_or_else(|| DEFAULT_DB_LOG.to_string());
    if path.trim().is_empty() {
        return Ok(());
    }
    let current = prepare_log_file(PathBuf::from(path), policy)?;
    create_private(&current)
        .with_context(|| messages::infra::logging::db::init_failed(current.display()))?;
    let writer = RotatingFile::open(current.clone(), *policy)
        .with_context(|| messages::infra::logging::db::init_failed(current.display()))?;
    if let Ok(mut guard) = DB_LOG.lock() {
        *guard = Some(writer);
    }
    DB_LOG_PATH.set(current).ok();
    Ok(())
}

pub fn append_db_log(message: &str) {
    let Ok(mut guard) = DB_LOG.lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    if let Ok(format) = format_description::parse("[year]-[month]-[day]T[hour]:[minute]:[second]Z")
    {
        if let Ok(ts) = OffsetDateTime::now_utc().format(&format) {
            let _ = writeln!(file, "{ts} {message}");
            return;
        }
    }
    let _ = writeln!(file, "{message}");
}
