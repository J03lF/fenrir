use anyhow::Result;
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter, Registry};

use crate::config::AppConfig;

pub fn init_tracing(cfg: &AppConfig) -> Result<()> {
    let level = cfg.telemetry.tracing_level.clone();
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true);
    let subscriber = Registry::default().with(env_filter).with(fmt_layer);
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}
