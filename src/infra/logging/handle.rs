use anyhow::{anyhow, Result};
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::reload::Handle;
use tracing_subscriber::Registry;

use crate::utils::messages;

#[derive(Clone)]
pub struct ReloadHandle {
    pub(super) inner: Handle<EnvFilter, Registry>,
}

impl ReloadHandle {
    pub fn reload_level(&self, level: &str) -> Result<()> {
        let filter = EnvFilter::try_new(level).map_err(|err| {
            anyhow!(
                "{}",
                messages::infra::logging::handle::invalid_level(level, err)
            )
        })?;
        self.inner
            .reload(filter)
            .map_err(|err| anyhow!("{}", messages::infra::logging::handle::reload_failed(err)))
    }
}
