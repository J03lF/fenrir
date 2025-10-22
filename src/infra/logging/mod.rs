use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use time::{format_description, OffsetDateTime};
use tracing::info;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload::{self, Handle};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

use crate::config::AppConfig;

const DEFAULT_APP_LOG: &str = "logs/app.log";
const DEFAULT_DB_LOG: &str = "logs/db.log";
const ARCHIVE_DIR: &str = "archive";
const MAX_ARCHIVES: usize = 3;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static DB_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone)]
pub struct ReloadHandle {
    inner: Handle<EnvFilter, Registry>,
}

impl ReloadHandle {
    pub fn reload_level(&self, level: &str) -> Result<()> {
        let filter = EnvFilter::try_new(level.to_string())
            .map_err(|err| anyhow!("ungültiger tracing level '{}': {err}", level))?;
        self.inner
            .reload(filter)
            .map_err(|err| anyhow!("konnte logging filter nicht aktualisieren: {err}"))
    }
}

pub fn init_tracing(cfg: &AppConfig) -> Result<ReloadHandle> {
    prepare_db_log()?;

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(cfg.telemetry.tracing.level.clone()))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter_layer, handle) = reload::Layer::new(filter);

    let base = tracing_subscriber::registry().with(filter_layer);

    if let Some((file_writer, guard, path)) = build_app_writer()? {
        LOG_GUARD.set(guard).ok();
        LOG_PATH.set(path.clone()).ok();
        let stdout_writer = io::stdout as fn() -> io::Stdout;
        let combined = file_writer.and(stdout_writer);
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(combined)
            .with_ansi(false);
        base.with(fmt_layer)
            .try_init()
            .map_err(|err| anyhow!("konnte Tracing-Subscriber nicht initialisieren: {err}"))?;
        info!(log_path = %path.display(), "dateilogging initialisiert");
    } else {
        let ansi = io::stdout().is_terminal();
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(io::stdout as fn() -> io::Stdout)
            .with_ansi(ansi);
        base.with(fmt_layer)
            .try_init()
            .map_err(|err| anyhow!("konnte Tracing-Subscriber nicht initialisieren: {err}"))?;
        info!("logging auf stdout initialisiert");
    }

    if let Some(path) = db_log_file_path() {
        info!(log_path = %path.display(), "db-logdatei vorbereitet");
    }

    Ok(ReloadHandle { inner: handle })
}

pub fn reload(handle: &ReloadHandle, level: &str) -> Result<()> {
    handle.reload_level(level)
}

pub fn log_file_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}

pub fn db_log_file_path() -> Option<PathBuf> {
    DB_LOG_PATH.get().cloned()
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

#[derive(Clone, Copy)]
pub enum LogKind {
    App,
    Db,
}

impl LogKind {
    fn prefix(&self) -> &'static str {
        match self {
            LogKind::App => "app",
            LogKind::Db => "db",
        }
    }
}

fn build_app_writer() -> Result<Option<(NonBlocking, WorkerGuard, PathBuf)>> {
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
        .ok_or_else(|| anyhow!("ungültiger Log-Pfad"))?
        .to_owned();
    let appender = tracing_appender::rolling::never(parent, file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    Ok(Some((non_blocking, guard, current)))
}

fn prepare_db_log() -> Result<()> {
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
        .with_context(|| format!("konnte DB-Log nicht initialisieren: {}", current.display()))?;
    DB_LOG_PATH.set(current).ok();
    Ok(())
}

fn prepare_log_file(path: PathBuf, kind: LogKind) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("konnte Log-Verzeichnis nicht anlegen: {}", parent.display())
        })?;
    }
    rotate_if_exists(&path, kind.prefix())?;
    Ok(path)
}

fn rotate_if_exists(current: &Path, prefix: &str) -> Result<()> {
    if !current.exists() {
        return Ok(());
    }
    let archive_dir = archive_dir_for(current);
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("konnte Log-Archiv nicht anlegen: {}", archive_dir.display()))?;
    let ts_format = format_description::parse("[year][month][day]_[hour][minute][second]")?;
    let timestamp = OffsetDateTime::now_utc().format(&ts_format)?;
    let archive_name = format!("{prefix}-{timestamp}.log");
    let archive_path = archive_dir.join(archive_name);
    fs::rename(current, &archive_path).with_context(|| {
        format!(
            "konnte Logdatei nicht in Archiv verschieben: {} -> {}",
            current.display(),
            archive_path.display()
        )
    })?;
    prune_archives(&archive_dir, prefix, MAX_ARCHIVES)?;
    Ok(())
}

fn prune_archives(dir: &Path, prefix: &str, keep: usize) -> Result<()> {
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

fn archive_dir_for(current: &Path) -> PathBuf {
    current
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(ARCHIVE_DIR)
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
