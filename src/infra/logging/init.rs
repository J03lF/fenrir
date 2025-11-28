use std::io::{self, IsTerminal};

use anyhow::{anyhow, Result};
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use crate::config::AppConfig;
use crate::utils::messages;

use super::db::prepare_db_log;
use super::handle::ReloadHandle;
use super::writers::build_app_writer;
use super::{db_log_file_path, LOG_GUARD, LOG_PATH};

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
        base.with(fmt_layer).try_init().map_err(|err| {
            anyhow!(
                "{}",
                messages::infra::logging::init::tracing_subscriber_init_failed(err)
            )
        })?;
        info!(
            log_path = %path.display(),
            "{}", messages::infra::logging::init::FILE_LOGGING_INITIALIZED
        );
    } else {
        let ansi = io::stdout().is_terminal();
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_writer(io::stdout as fn() -> io::Stdout)
            .with_ansi(ansi);
        base.with(fmt_layer).try_init().map_err(|err| {
            anyhow!(
                "{}",
                messages::infra::logging::init::tracing_subscriber_init_failed(err)
            )
        })?;
        info!(
            "{}",
            messages::infra::logging::init::STDOUT_LOGGING_INITIALIZED
        );
    }

    if let Some(path) = db_log_file_path() {
        info!(
            log_path = %path.display(),
            "{}", messages::infra::logging::init::DB_LOG_PREPARED
        );
    }

    Ok(ReloadHandle { inner: handle })
}
