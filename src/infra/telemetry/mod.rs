use crate::config::AppConfig;
use anyhow::Result;

pub fn init(_cfg: &AppConfig) -> Result<()> {
    // Placeholder for metrics exporter and health endpoints (optional HTTP later)
    Ok(())
}

pub fn is_ready() -> bool {
    true
}
pub fn is_live() -> bool {
    true
}
