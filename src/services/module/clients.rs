use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::Value as JsonValue;
use tokio::time::sleep;

use crate::config::{
    ConfigError, ModuleRuntimeClientSection, ModuleRuntimeClientTlsSection,
    ModuleRuntimeRolloutSection,
};

#[derive(Clone, Debug)]
pub struct ModuleClientSettings {
    pub timeout: Duration,
    pub retries: u32,
    pub backoff: Duration,
    pub health_interval: Duration,
    pub tls: ModuleClientTlsSettings,
}

#[derive(Clone, Debug)]
pub struct ModuleRolloutSettings {
    pub drain_before_restart: Duration,
    pub inter_restart_delay: Duration,
    pub health_check_timeout: Duration,
    pub health_poll_interval: Duration,
    pub rollback_on_failure: bool,
    pub abort_on_first_failure: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleClientTlsSettings {
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    pub accept_invalid_certs: bool,
}

impl ModuleClientSettings {
    pub fn from_config(cfg: &ModuleRuntimeClientSection) -> Result<Self, ConfigError> {
        Ok(Self {
            timeout: Duration::from_millis(cfg.timeout_ms),
            retries: cfg.retries,
            backoff: Duration::from_millis(cfg.backoff_ms),
            health_interval: Duration::from_secs(cfg.health_probe_interval_seconds),
            tls: ModuleClientTlsSettings::from_config(&cfg.tls)?,
        })
    }
}

impl ModuleRolloutSettings {
    pub fn from_config(cfg: &ModuleRuntimeRolloutSection) -> Self {
        Self {
            drain_before_restart: Duration::from_millis(cfg.drain_before_restart_ms),
            inter_restart_delay: Duration::from_millis(cfg.inter_restart_delay_ms),
            health_check_timeout: Duration::from_millis(cfg.health_check_timeout_ms),
            health_poll_interval: Duration::from_millis(cfg.health_poll_interval_ms),
            rollback_on_failure: cfg.rollback_on_failure,
            abort_on_first_failure: cfg.abort_on_first_failure,
        }
    }
}

impl ModuleClientTlsSettings {
    fn from_config(cfg: &ModuleRuntimeClientTlsSection) -> Result<Self, ConfigError> {
        Ok(Self {
            ca_cert_path: cfg
                .ca_cert_path
                .as_ref()
                .map(|value| canonicalize_path(value))
                .transpose()?,
            client_cert_path: cfg
                .client_cert_path
                .as_ref()
                .map(|value| canonicalize_path(value))
                .transpose()?,
            client_key_path: cfg
                .client_key_path
                .as_ref()
                .map(|value| canonicalize_path(value))
                .transpose()?,
            accept_invalid_certs: cfg.accept_invalid_certs,
        })
    }
}

fn canonicalize_path(value: &str) -> Result<String, ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::InvalidMessage(
            "tls path references must not be empty".to_string(),
        ));
    }
    let path = PathBuf::from(trimmed);
    if !path.exists() {
        return Err(ConfigError::InvalidMessage(format!(
            "tls path {trimmed} does not exist"
        )));
    }
    fs::canonicalize(&path)
        .map_err(|err| {
            ConfigError::InvalidMessage(format!("failed to canonicalize tls path {trimmed}: {err}"))
        })
        .map(|resolved| resolved.to_string_lossy().to_string())
}

#[derive(Clone)]
pub struct ModuleHealthHttpClient {
    inner: Client,
    retries: u32,
    backoff: Duration,
}

impl ModuleHealthHttpClient {
    pub fn new(settings: &ModuleClientSettings) -> Result<Self, ConfigError> {
        let client = Client::builder()
            .timeout(settings.timeout)
            .build()
            .map_err(|err| {
                ConfigError::InvalidMessage(format!("failed to initialize health client: {err}"))
            })?;
        Ok(Self {
            inner: client,
            retries: settings.retries,
            backoff: settings.backoff,
        })
    }

    pub async fn check(&self, url: &str) -> Result<(), reqwest::Error> {
        let mut attempts = 0;
        loop {
            match self
                .inner
                .get(url)
                .header("x-fenrir-health-probe", "control-plane")
                .send()
                .await
            {
                Ok(response) => return response.error_for_status().map(|_| ()),
                Err(err) => {
                    attempts += 1;
                    if attempts > self.retries {
                        return Err(err);
                    }
                    let delay = self.backoff.saturating_mul(attempts);
                    sleep(delay).await;
                }
            }
        }
    }

    pub async fn fetch_json(&self, url: &str) -> Result<JsonValue, reqwest::Error> {
        let mut attempts = 0;
        loop {
            match self
                .inner
                .get(url)
                .header("x-fenrir-health-probe", "control-plane")
                .send()
                .await
            {
                Ok(response) => {
                    let response = response.error_for_status()?;
                    return response.json::<JsonValue>().await;
                }
                Err(err) => {
                    attempts += 1;
                    if attempts > self.retries {
                        return Err(err);
                    }
                    let delay = self.backoff.saturating_mul(attempts);
                    sleep(delay).await;
                }
            }
        }
    }
}
