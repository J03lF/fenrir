use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use tokio::time::sleep;

use crate::config::{ConfigError, ModuleRuntimeClientSection, ModuleRuntimeClientTlsSection};

#[derive(Clone, Debug)]
pub struct ModuleClientSettings {
    pub timeout: Duration,
    pub retries: u32,
    pub backoff: Duration,
    pub health_interval: Duration,
    pub tls: ModuleClientTlsSettings,
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
}
