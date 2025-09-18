use crate::config::{self, AppConfig};
use crate::infra::{logging, ssh, telemetry};
use anyhow::Result;

pub struct BootContext {
    pub config: AppConfig,
}

pub fn boot() -> Result<BootContext> {
    let cfg = config::load()?;
    logging::init_tracing(&cfg)?;
    telemetry::init(&cfg)?;
    tracing::info!(app = %cfg.app.name, version = %cfg.app.version, "boot complete");
    Ok(BootContext { config: cfg })
}

pub async fn start_transports(ctx: &BootContext) -> Result<()> {
    // Start SSH server (blocking future) on its own task
    let cfg = ctx.config.clone();
    tokio::spawn(async move {
        if let Err(e) = ssh::start(&cfg).await {
            tracing::error!(error=%e, "ssh server exited with error");
        }
    });
    Ok(())
}
