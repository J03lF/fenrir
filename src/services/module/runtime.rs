use std::{
    env,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use crate::domain::module::{
    ModuleId, ModuleManifest, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimeKind,
    ModuleRuntimeStatus, ModuleStartConfig, ModuleVersion,
};
use crate::security::service::{ServiceRole, ServiceScope};
use crate::security::service_tokens::DelegatedToken;
use crate::services::{
    ServiceDescriptorOwned, ServiceIngressMetadata, ServiceIngressProtocol, ServiceKind,
    ServiceSecurityMetadata, ServiceStatus, ServiceTag,
};
use crate::utils::messages::services::module::{
    runtime::{logs as runtime_logs, notes as runtime_notes},
    service::logs as module_service_logs,
};
use reqwest::StatusCode;
use serde::Serialize;
use tokio::fs;
use tokio::time::sleep;

use super::config::ModuleEnvResolutionError;
use super::reported::{ReportedServiceEntry, ReportedServicesPayload};
use super::service::{MODULE_SERVICE_MANIFEST_PATH, RESERVED_ENV_KEYS};
use super::{ModuleClientSettings, ModuleService};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const MANIFEST_REFRESH_MAX_ATTEMPTS: u32 = 20;
const MANIFEST_REFRESH_MIN_DELAY_MS: u64 = 500;

impl ModuleService {
    pub async fn ensure_all_running(&self) {
        self.diagnostics.record_heartbeat("module-lifecycle");
        let modules = match self.list_installed().await {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!(error = %err, "{}", runtime_logs::LIST_FOR_AUTOSTART_FAILED);
                return;
            }
        };

        for module in modules {
            let Ok(module_id) = module.manifest.module_id() else {
                continue;
            };
            if let Err(err) = self.ensure_running(&module_id).await {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    runtime_logs::AUTOSTART_FAILED
                );
            }
        }
    }

    pub async fn stop_all_modules(&self) -> Result<(), ModuleRuntimeError> {
        let running = self.runtime.list_running().await?;
        for info in running {
            let started_at = Instant::now();
            let stop_result = self.runtime.stop(&info.module_id).await;
            let success = stop_result
                .as_ref()
                .map(|_| true)
                .unwrap_or_else(|err| matches!(err, ModuleRuntimeError::NotRunning { .. }));
            self.record_lifecycle_metrics(started_at, success);
            if let Err(err) = stop_result {
                if !matches!(err, ModuleRuntimeError::NotRunning { .. }) {
                    tracing::warn!(
                        module = %info.module_id,
                        error = %err,
                        "{}",
                        runtime_logs::STOP_DURING_SYNC_FAILED
                    );
                }
            }
            self.revoke_service_token_if_any(&info.module_id, "module-stop-all")
                .await;
            self.stop_gateway_if_any(&info.module_id).await;
        }
        Ok(())
    }

    pub(super) async fn stop_module_process(&self, module_id: &ModuleId) {
        let started_at = Instant::now();
        let stop_result = self.runtime.stop(module_id).await;
        let success = stop_result
            .as_ref()
            .map(|_| true)
            .unwrap_or_else(|err| matches!(err, ModuleRuntimeError::NotRunning { .. }));
        if let Err(err) = stop_result {
            if !matches!(err, ModuleRuntimeError::NotRunning { .. }) {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    runtime_logs::STOP_BEFORE_UPDATE_FAILED
                );
            }
        }
        self.record_lifecycle_metrics(started_at, success);
        self.revoke_service_token_if_any(module_id, "module-stop")
            .await;
        self.clear_reported_services(module_id).await;
        self.stop_gateway_if_any(module_id).await;
    }

    pub async fn ensure_running(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        self.diagnostics.record_heartbeat("module-lifecycle");
        self.diagnostics.record_heartbeat("module-runtime");
        let installed = match self.storage.load(module_id).await {
            Ok(Some(installed)) => installed,
            Ok(None) => return Ok(()),
            Err(err) => {
                return Err(ModuleRuntimeError::InvalidState(err.to_string()));
            }
        };

        if self.is_unmanaged_module(module_id) {
            return Ok(());
        }

        self.guard_quarantine(module_id)?;

        self.register_declared_services(module_id, &installed)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        match self.runtime.status(module_id).await {
            Ok(info) if matches!(info.status, ModuleRuntimeStatus::Running) => {
                if let Err(err) = self.ensure_gateway_endpoint(module_id).await {
                    tracing::warn!(
                        module = %module_id,
                        error = %err,
                        "failed to ensure runtime gateway"
                    );
                }
                self.update_module_service_status(
                    module_id,
                    &installed.manifest,
                    ServiceStatus::Active,
                    Some(Self::runtime_status_note(&info)),
                );
                match self
                    .refresh_reported_services(module_id, &installed.manifest, info.port)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        self.schedule_manifest_refresh(module_id).await;
                    }
                    Err(err) => {
                        tracing::debug!(
                            module = %module_id,
                            error = %err,
                            "failed to refresh module service manifest"
                        );
                        self.schedule_manifest_refresh(module_id).await;
                    }
                }
                Ok(())
            }
            Ok(_) | Err(ModuleRuntimeError::NotRunning { .. }) => {
                let config = ModuleStartConfig {
                    module_id: module_id.clone(),
                    port: None,
                    env_vars: Vec::new(),
                    auto_restart: true,
                };
                self.start(config).await.map(|_| ())
            }
            Err(err) => Err(err),
        }
    }

    pub async fn start_module(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let config = ModuleStartConfig {
            module_id: module_id.clone(),
            port: None,
            env_vars: Vec::new(),
            auto_restart: true,
        };
        self.start(config).await
    }

    pub async fn start(
        &self,
        mut config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let installed = self
            .storage
            .load(&config.module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: config.module_id.to_string(),
            })?;

        if self.is_unmanaged_module(&config.module_id) {
            return Ok(ModuleRuntimeInfo {
                module_id: config.module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Stopped,
                kind: ModuleRuntimeKind::Process,
                pid: None,
                port: config.port,
                started_at: None,
                stopped_at: Some(SystemTime::now()),
                restart_count: 0,
            });
        }

        self.register_declared_services(&config.module_id, &installed)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if self.is_dev_override_active(&config.module_id).await {
            return Ok(ModuleRuntimeInfo {
                module_id: config.module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Running,
                kind: ModuleRuntimeKind::Process,
                pid: None,
                port: config.port,
                started_at: Some(SystemTime::now()),
                stopped_at: None,
                restart_count: 0,
            });
        }

        if let Ok(info) = self.runtime.status(&config.module_id).await {
            if matches!(
                info.status,
                crate::domain::module::ModuleRuntimeStatus::Running
            ) {
                return Err(ModuleRuntimeError::AlreadyRunning {
                    module_id: config.module_id.to_string(),
                });
            }
        }

        self.guard_quarantine(&config.module_id)?;

        let gateway_endpoint = self.ensure_gateway_endpoint(&config.module_id).await?;

        let issued_token = self.issue_module_service_token(&config.module_id).await?;

        let assigned_port = match config.port {
            Some(port) => Some(port),
            None => self.port_allocator.assigned_port(&config.module_id).await?,
        };
        config.port = assigned_port;
        config.env_vars = self.inject_runtime_env(
            config.env_vars,
            &config.module_id,
            assigned_port,
            issued_token.as_ref(),
        )?;
        config
            .env_vars
            .retain(|(key, _)| key != "FENRIR_GATEWAY_ENDPOINT");
        config.env_vars.push((
            "FENRIR_GATEWAY_ENDPOINT".to_string(),
            gateway_endpoint.clone(),
        ));

        let manifest = installed.manifest.clone();
        let module_id_clone = config.module_id.clone();
        let runtime_started_at = Instant::now();
        let runtime_result = self.runtime.start(config).await;
        let runtime_success = runtime_result.is_ok();
        self.record_lifecycle_metrics(runtime_started_at, runtime_success);
        let runtime_info = match runtime_result {
            Ok(info) => {
                self.clear_health(&info.module_id);
                info
            }
            Err(err) => {
                self.stop_gateway_if_any(&module_id_clone).await;
                let final_err = if let Some(until) = self.record_failure(&module_id_clone) {
                    self.annotate_quarantine(&module_id_clone, &manifest, until);
                    ModuleRuntimeError::Quarantined {
                        module_id: module_id_clone.to_string(),
                        resume_at: until,
                    }
                } else {
                    err
                };
                if let Some(token) = issued_token.as_ref() {
                    if let Err(revoke_err) = self
                        .security
                        .revoke_service_token(&token.token, "module-start-failed")
                    {
                        tracing::warn!(
                            module = %token.claims.actor.identifier(),
                            error = %revoke_err,
                            "{}",
                            module_service_logs::SERVICE_TOKEN_REVOKE_FAILED
                        );
                    }
                }
                return Err(final_err);
            }
        };

        if let Some(token) = issued_token {
            self.audit_runtime_token_refresh(&runtime_info.module_id, &token);
            self.record_service_token(&runtime_info.module_id, &token)
                .await;
        }

        tracing::info!(
            module_id = %runtime_info.module_id,
            pid = ?runtime_info.pid,
            port = ?runtime_info.port,
            "{}",
            runtime_logs::MODULE_STARTED
        );

        self.update_module_service_status(
            &runtime_info.module_id,
            &manifest,
            ServiceStatus::Active,
            Some(Self::runtime_status_note(&runtime_info)),
        );

        match self
            .refresh_reported_services(&runtime_info.module_id, &manifest, runtime_info.port)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                self.schedule_manifest_refresh(&runtime_info.module_id)
                    .await;
            }
            Err(err) => {
                tracing::debug!(
                    module = %runtime_info.module_id,
                    error = %err,
                    "failed to refresh module service manifest"
                );
                self.schedule_manifest_refresh(&runtime_info.module_id)
                    .await;
            }
        }

        Ok(runtime_info)
    }

    pub async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        if self.is_unmanaged_module(module_id) || self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        let stop_started_at = Instant::now();
        let stop_result = self.runtime.stop(module_id).await;
        let stop_success = stop_result.is_ok();
        self.record_lifecycle_metrics(stop_started_at, stop_success);
        stop_result?;

        tracing::info!(module_id = %module_id, "{}", runtime_logs::MODULE_STOPPED);

        if let Some(installed) = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
        {
            self.update_module_service_status(
                module_id,
                &installed.manifest,
                ServiceStatus::Stopped,
                Some(runtime_notes::STOPPED.to_string()),
            );
        }

        self.revoke_service_token_if_any(module_id, "module-stop")
            .await;
        self.clear_health(module_id);
        self.clear_reported_services(module_id).await;
        self.stop_gateway_if_any(module_id).await;
        Ok(())
    }

    pub async fn runtime_status(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        self.runtime.status(module_id).await
    }

    pub async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        self.runtime.list_running().await
    }

    pub async fn restart(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        tracing::info!(module_id = %module_id, "{}", runtime_logs::RESTARTING_MODULE);
        self.guard_quarantine(module_id)?;
        let installed = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id.to_string(),
            })?;

        if self.is_unmanaged_module(module_id) {
            return Ok(ModuleRuntimeInfo {
                module_id: module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Stopped,
                kind: ModuleRuntimeKind::Process,
                pid: None,
                port: None,
                started_at: None,
                stopped_at: Some(SystemTime::now()),
                restart_count: 0,
            });
        }

        if self.is_dev_override_active(module_id).await {
            return Ok(ModuleRuntimeInfo {
                module_id: module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Running,
                kind: ModuleRuntimeKind::Process,
                pid: None,
                port: None,
                started_at: None,
                stopped_at: None,
                restart_count: 1,
            });
        }

        let restart_count = self
            .runtime
            .status(module_id)
            .await
            .map(|info| info.restart_count.saturating_add(1))
            .unwrap_or(0);

        self.stop(module_id).await?;

        let mut info = self
            .start(ModuleStartConfig {
                module_id: module_id.clone(),
                port: None,
                env_vars: vec![],
                auto_restart: false,
            })
            .await?;
        info.restart_count = restart_count;
        self.update_module_service_status(
            module_id,
            &installed.manifest,
            ServiceStatus::Active,
            Some(Self::runtime_status_note(&info)),
        );
        Ok(info)
    }

    pub async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError> {
        self.runtime.logs(module_id, tail).await
    }

    pub async fn register_reported_services(
        &self,
        module_id: &ModuleId,
        payload: ReportedServicesPayload,
    ) -> Result<(), ModuleRuntimeError> {
        let installed = self
            .storage
            .load(module_id)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id.to_string(),
            })?;
        if self.is_dev_override_active(module_id).await {
            return Ok(());
        }
        let info = self.runtime.status(module_id).await?;
        if !matches!(info.status, ModuleRuntimeStatus::Running) {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id.to_string(),
            });
        }
        let Some(port) = info.port else {
            return Err(ModuleRuntimeError::InvalidState(format!(
                "module {module_id} has no runtime port"
            )));
        };
        let _ = self
            .apply_reported_services(module_id, &installed.manifest, port, payload)
            .await?;
        Ok(())
    }

    pub async fn runtime_env(
        &self,
        module_id: &ModuleId,
    ) -> Result<Vec<(String, String)>, ModuleRuntimeError> {
        self.runtime.env(module_id).await
    }

    pub(super) async fn ensure_gateway_endpoint(
        &self,
        module_id: &ModuleId,
    ) -> Result<String, ModuleRuntimeError> {
        let host = self.self_ref.clone();
        self.runtime_gateways.ensure(host, module_id).await
    }

    pub(super) async fn stop_gateway_if_any(&self, module_id: &ModuleId) {
        self.runtime_gateways.stop(module_id).await;
    }

    pub(super) async fn active_service_token_value(&self, module_id: &ModuleId) -> Option<String> {
        let guard = self.service_tokens.read().await;
        guard.get(module_id).map(|lease| lease.token().to_string())
    }

    pub(crate) fn inject_runtime_env(
        &self,
        mut env: Vec<(String, String)>,
        module_id: &ModuleId,
        port: Option<u16>,
        service_token: Option<&DelegatedToken>,
    ) -> Result<Vec<(String, String)>, ModuleRuntimeError> {
        env.retain(|(key, _)| !RESERVED_ENV_KEYS.contains(&key.as_str()));

        let module_id_str = module_id.to_string();
        let service_id = Self::module_service_id(module_id);

        env.push(("FENRIR_MODULE_ID".to_string(), module_id_str));
        env.push(("FENRIR_SERVICE_ID".to_string(), service_id.clone()));
        env.push((
            "FENRIR_SERVICE_URI".to_string(),
            format!("service://{}", service_id),
        ));
        if let Some(port) = port {
            env.push(("FENRIR_SERVICE_PORT".to_string(), port.to_string()));
            env.push((
                "FENRIR_SERVICE_ADDR".to_string(),
                format!("127.0.0.1:{port}"),
            ));
        }
        if let Some(token) = service_token {
            self.append_service_token_env(&mut env, token);
        }
        self.append_db_connector_env(&mut env);
        self.append_control_plane_env(&mut env);
        self.append_service_snapshot_env(&mut env);
        self.append_client_env(&mut env);
        self.append_otel_env(&mut env, module_id, &service_id);
        self.append_configured_env(&mut env, &service_id)?;
        Ok(env)
    }

    fn append_db_connector_env(&self, env: &mut Vec<(String, String)>) {
        let endpoint = match self.db_connector_endpoint.read() {
            Ok(guard) => (*guard).clone(),
            Err(_) => None,
        };
        if let Some(endpoint) = endpoint {
            env.push((
                "FENRIR_DB_CONNECTOR_PROTOCOL".to_string(),
                endpoint.protocol().to_string(),
            ));
            env.push((
                "FENRIR_DB_CONNECTOR_ENDPOINT".to_string(),
                endpoint.location(),
            ));
            env.push(("FENRIR_DB_CONNECTOR_URI".to_string(), endpoint.uri()));
        }
    }

    fn append_control_plane_env(&self, env: &mut Vec<(String, String)>) {
        let url = match self.control_plane_url.read() {
            Ok(guard) => guard.clone(),
            Err(_) => None,
        };
        if let Some(url) = url {
            env.push(("FENRIR_CONTROL_PLANE_URL".to_string(), url));
        }
    }

    fn append_service_snapshot_env(&self, env: &mut Vec<(String, String)>) {
        if let Some(path) = &self.service_snapshot_path {
            self.replace_env(
                env,
                "FENRIR_SERVICE_SNAPSHOT_PATH",
                path.to_string_lossy().to_string(),
            );
        }
    }

    fn append_service_token_env(&self, env: &mut Vec<(String, String)>, token: &DelegatedToken) {
        env.push(("FENRIR_SERVICE_TOKEN".to_string(), token.token.clone()));
        if let Some(value) = Self::format_timestamp(token.claims.issued_at) {
            env.push(("FENRIR_SERVICE_TOKEN_ISSUED_AT".to_string(), value));
        }
        if let Some(value) = Self::format_timestamp(token.claims.expires_at) {
            env.push(("FENRIR_SERVICE_TOKEN_EXPIRES_AT".to_string(), value));
        }
        env.push((
            "FENRIR_SERVICE_TOKEN_TTL_SECS".to_string(),
            Self::remaining_token_ttl_seconds(token).to_string(),
        ));
    }

    fn append_client_env(&self, env: &mut Vec<(String, String)>) {
        self.replace_env(
            env,
            "FENRIR_CONTROL_PLANE_TIMEOUT_MS",
            self.client_settings.timeout.as_millis().to_string(),
        );
        self.replace_env(
            env,
            "FENRIR_CONTROL_PLANE_RETRY_ATTEMPTS",
            self.client_settings.retries.to_string(),
        );
        self.replace_env(
            env,
            "FENRIR_CONTROL_PLANE_RETRY_BACKOFF_MS",
            self.client_settings.backoff.as_millis().to_string(),
        );
        self.replace_env(
            env,
            "FENRIR_CONTROL_PLANE_TLS_ACCEPT_INVALID",
            self.client_settings.tls.accept_invalid_certs.to_string(),
        );
        if let Some(path) = &self.client_settings.tls.ca_cert_path {
            self.replace_env(env, "FENRIR_CONTROL_PLANE_TLS_CA_CERT", path.clone());
        }
        if let Some(path) = &self.client_settings.tls.client_cert_path {
            self.replace_env(env, "FENRIR_CONTROL_PLANE_TLS_CLIENT_CERT", path.clone());
        }
        if let Some(path) = &self.client_settings.tls.client_key_path {
            self.replace_env(env, "FENRIR_CONTROL_PLANE_TLS_CLIENT_KEY", path.clone());
        }
    }

    fn append_configured_env(
        &self,
        env: &mut Vec<(String, String)>,
        service_id: &str,
    ) -> Result<(), ModuleRuntimeError> {
        let overrides = self.overrides.env_for(service_id);
        if overrides.is_empty() {
            return Ok(());
        }
        for var in overrides {
            let (key, value) = var.resolve_for(service_id).map_err(Self::map_env_error)?;
            if RESERVED_ENV_KEYS.contains(&key.as_str()) {
                continue;
            }
            env.retain(|(existing, _)| existing != &key);
            env.push((key, value));
        }
        Ok(())
    }

    fn append_otel_env(
        &self,
        env: &mut Vec<(String, String)>,
        module_id: &ModuleId,
        service_id: &str,
    ) {
        if !env.iter().any(|(key, _)| key == "OTEL_SERVICE_NAME") {
            env.push(("OTEL_SERVICE_NAME".to_string(), service_id.to_string()));
        }
        let resource_value = format!("service.name={service_id},fenrir.module_id={}", module_id);
        match env
            .iter_mut()
            .find(|(key, _)| key == "OTEL_RESOURCE_ATTRIBUTES")
        {
            Some((_, value)) => {
                if !value.trim().is_empty() {
                    value.push(',');
                }
                value.push_str(&resource_value);
            }
            None => env.push(("OTEL_RESOURCE_ATTRIBUTES".to_string(), resource_value)),
        }
        for name in [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_TRACES_EXPORTER",
            "OTEL_EXPORTER_OTLP_CERTIFICATE",
            "TRACEPARENT",
            "TRACESTATE",
        ] {
            if env.iter().any(|(key, _)| key == name) {
                continue;
            }
            if let Ok(value) = env::var(name) {
                if !value.trim().is_empty() {
                    env.push((name.to_string(), value));
                }
            }
        }
    }

    fn map_env_error(err: ModuleEnvResolutionError) -> ModuleRuntimeError {
        ModuleRuntimeError::InvalidState(err.to_message())
    }

    fn replace_env(&self, env: &mut Vec<(String, String)>, key: &str, value: String) {
        env.retain(|(existing, _)| existing != key);
        env.push((key.to_string(), value));
    }

    fn runtime_status_note(info: &ModuleRuntimeInfo) -> String {
        match info.kind {
            ModuleRuntimeKind::StaticSite => info
                .port
                .map(runtime_notes::running_static)
                .unwrap_or_else(|| runtime_notes::RUNNING.to_string()),
            ModuleRuntimeKind::Process => info
                .pid
                .map(runtime_notes::running_with_pid)
                .unwrap_or_else(|| runtime_notes::RUNNING.to_string()),
        }
    }

    fn format_timestamp(value: OffsetDateTime) -> Option<String> {
        value.format(&Rfc3339).ok()
    }

    fn remaining_token_ttl_seconds(token: &DelegatedToken) -> u64 {
        let now = OffsetDateTime::now_utc();
        if token.claims.expires_at <= now {
            return 0;
        }
        (token.claims.expires_at - now).whole_seconds().max(0) as u64
    }

    pub(super) async fn refresh_reported_services(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        port: Option<u16>,
    ) -> Result<bool, ModuleRuntimeError> {
        let Some(port) = port else {
            self.clear_reported_services(module_id).await;
            return Ok(false);
        };

        let attempt_count = self.client_settings.retries.saturating_add(1).max(2);
        let mut attempt = 0u32;
        let mut last_error: Option<String> = None;

        while attempt < attempt_count {
            match self.fetch_reported_services(module_id, port).await {
                Ok(Some(payload)) => {
                    if self
                        .apply_reported_services(module_id, manifest, port, payload)
                        .await?
                    {
                        return Ok(true);
                    }
                    break;
                }
                Ok(None) => {
                    if attempt + 1 < attempt_count {
                        tracing::debug!(
                            module = %module_id,
                            attempt = attempt + 1,
                            "module service manifest not yet reachable, retrying"
                        );
                        sleep(self.client_settings.backoff).await;
                        attempt += 1;
                        continue;
                    }
                }
                Err(err) => {
                    let message = err.to_string();
                    if attempt + 1 < attempt_count {
                        tracing::debug!(
                            module = %module_id,
                            error = %message,
                            attempt = attempt + 1,
                            "failed to fetch module service manifest, retrying"
                        );
                        last_error = Some(message);
                        sleep(self.client_settings.backoff).await;
                        attempt += 1;
                        continue;
                    } else {
                        tracing::debug!(
                            module = %module_id,
                            error = %message,
                            "giving up on module service manifest fetch"
                        );
                        last_error = Some(message);
                    }
                }
            }
            break;
        }

        if let Some(message) = last_error {
            tracing::debug!(
                module = %module_id,
                error = %message,
                "falling back to module placeholder service descriptor"
            );
        }

        self.ensure_module_service_entry(module_id, manifest);
        Ok(false)
    }

    pub(super) async fn schedule_manifest_refresh(&self, module_id: &ModuleId) {
        if self.is_dev_override_active(module_id).await {
            return;
        }
        let Some(service) = self.self_ref.upgrade() else {
            return;
        };
        let module_id = module_id.clone();
        let mut tasks = self.manifest_refresh_tasks.lock().await;
        if tasks.contains_key(&module_id) {
            return;
        }
        let settings = self.client_settings.clone();
        let service_clone = Arc::clone(&service);
        let module_id_clone = module_id.clone();
        let handle = tokio::spawn(async move {
            service_clone
                .run_manifest_refresh_loop(module_id_clone, settings)
                .await;
        });
        tasks.insert(module_id, handle);
    }

    async fn run_manifest_refresh_loop(
        self: Arc<Self>,
        module_id: ModuleId,
        settings: ModuleClientSettings,
    ) {
        let delay = settings
            .backoff
            .max(Duration::from_millis(MANIFEST_REFRESH_MIN_DELAY_MS));
        for attempt in 0..MANIFEST_REFRESH_MAX_ATTEMPTS {
            sleep(delay).await;
            if self.is_dev_override_active(&module_id).await {
                break;
            }
            let installed = match self.storage.load(&module_id).await {
                Ok(Some(installed)) => installed,
                Ok(None) => break,
                Err(err) => {
                    tracing::debug!(
                        module = %module_id,
                        error = %err,
                        "manifest refresh failed to load module state"
                    );
                    continue;
                }
            };
            let manifest = installed.manifest.clone();
            let runtime_info = match self.runtime.status(&module_id).await {
                Ok(info) => info,
                Err(ModuleRuntimeError::NotRunning { .. }) => break,
                Err(err) => {
                    tracing::debug!(
                        module = %module_id,
                        error = %err,
                        "manifest refresh failed to read runtime status"
                    );
                    continue;
                }
            };
            if !matches!(runtime_info.status, ModuleRuntimeStatus::Running) {
                break;
            }
            let Some(port) = runtime_info.port else {
                continue;
            };
            match self
                .refresh_reported_services(&module_id, &manifest, Some(port))
                .await
            {
                Ok(true) => {
                    tracing::info!(
                        module = %module_id,
                        attempt = attempt + 1,
                        "module service manifest registered after retry"
                    );
                    let mut guard = self.manifest_refresh_tasks.lock().await;
                    guard.remove(&module_id);
                    return;
                }
                Ok(false) => continue,
                Err(err) => {
                    tracing::debug!(
                        module = %module_id,
                        error = %err,
                        "manifest refresh retry failed"
                    );
                }
            }
        }
        let mut guard = self.manifest_refresh_tasks.lock().await;
        guard.remove(&module_id);
    }

    async fn fetch_reported_services(
        &self,
        module_id: &ModuleId,
        port: u16,
    ) -> Result<Option<ReportedServicesPayload>, ModuleRuntimeError> {
        let url = format!("http://127.0.0.1:{port}{MODULE_SERVICE_MANIFEST_PATH}");
        match self.manifest_client.get(&url).send().await {
            Ok(response) => {
                if response.status() == StatusCode::NOT_FOUND {
                    return Ok(None);
                }
                if !response.status().is_success() {
                    return Err(ModuleRuntimeError::InvalidState(format!(
                        "module {module_id} manifest fetch failed with status {}",
                        response.status()
                    )));
                }
                response
                    .json::<ReportedServicesPayload>()
                    .await
                    .map(Some)
                    .map_err(|err| {
                        ModuleRuntimeError::InvalidState(format!(
                            "module {module_id} manifest invalid: {err}"
                        ))
                    })
            }
            Err(err) if err.is_connect() || err.is_timeout() => Ok(None),
            Err(err) => Err(ModuleRuntimeError::InvalidState(format!(
                "module {module_id} manifest fetch failed: {err}"
            ))),
        }
    }

    async fn apply_reported_services(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        port: u16,
        payload: ReportedServicesPayload,
    ) -> Result<bool, ModuleRuntimeError> {
        if payload.services.is_empty() {
            self.clear_reported_services(module_id).await;
            return Ok(false);
        }

        self.clear_reported_services(module_id).await;

        let mut registered = Vec::new();
        let mut endpoints = Vec::new();

        for entry in payload.services {
            match self
                .build_descriptor_from_report(module_id, manifest, port, entry)
                .await
            {
                Ok((descriptor, endpoint, suffix)) => {
                    let descriptor_id = descriptor.id().to_string();
                    self.service_registry.register(
                        descriptor,
                        ServiceStatus::Active,
                        Some(format!("endpoint {endpoint}")),
                    );
                    registered.push(descriptor_id.clone());
                    endpoints.push((
                        Self::module_runtime_service_uri(module_id, &suffix),
                        endpoint,
                    ));
                }
                Err(err) => {
                    tracing::warn!(
                        module = %module_id,
                        error = %err,
                        "skipping invalid reported service entry"
                    );
                }
            }
        }

        if registered.is_empty() {
            return Ok(false);
        }

        {
            let mut guard = self.runtime_services.write().await;
            guard.insert(module_id.clone(), registered);
        }

        {
            let prefix = format!("service://module:{}::", module_id);
            let mut guard = self.service_endpoints.write().await;
            guard.retain(|uri, _| !uri.starts_with(&prefix));
            for (uri, endpoint) in endpoints {
                guard.insert(uri, endpoint);
            }
        }

        self.persist_service_snapshot().await;
        Ok(true)
    }

    async fn build_descriptor_from_report(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        port: u16,
        entry: ReportedServiceEntry,
    ) -> Result<(ServiceDescriptorOwned, String, String), ModuleRuntimeError> {
        if entry.service_id.trim().is_empty() {
            return Err(ModuleRuntimeError::InvalidState(
                "reported service id must not be empty".to_string(),
            ));
        }
        let suffix = entry.service_id.trim();
        let suffix_owned = suffix.to_string();
        let descriptor_id = Self::module_runtime_service_descriptor_id(module_id, suffix);
        let display_name = entry
            .name
            .clone()
            .unwrap_or_else(|| format!("{}::{suffix}", module_id));
        let description = entry
            .description
            .clone()
            .or_else(|| manifest.description.clone())
            .unwrap_or_else(|| format!("service {suffix} of module {module_id}"));
        let mut descriptor = ServiceDescriptorOwned::new(
            descriptor_id.clone(),
            display_name,
            description,
            map_kind(entry.kind.as_deref()),
        );
        if !entry.tags.is_empty() {
            descriptor = descriptor.with_tags(map_tags(&entry.tags));
        }

        let mut ingress = if entry
            .internal_only
            .unwrap_or_else(|| entry.ingress_access.as_deref() != Some("public"))
        {
            ServiceIngressMetadata::internal()
        } else {
            ServiceIngressMetadata::public()
        };
        if let Some(prefix) = entry.route_prefix.as_deref() {
            ingress = ingress.with_route_prefix(prefix);
        }
        if let Some(health) = entry.health_path.as_deref() {
            ingress = ingress.with_health_endpoint(health);
        }
        if !entry.protocols.is_empty() {
            ingress = ingress.with_protocols(map_protocols(&entry.protocols));
        }
        descriptor = descriptor.with_ingress(ingress);

        let mut security = if entry.internal_only.unwrap_or(true) {
            ServiceSecurityMetadata::internal_default()
        } else {
            let mut metadata = ServiceSecurityMetadata::internal_default();
            metadata.internal_only = false;
            metadata
        };
        if !entry.allowed_roles.is_empty() {
            security.allowed_roles = map_roles(&entry.allowed_roles);
        }
        if !entry.required_scopes.is_empty() {
            security.required_scopes = map_scopes(&entry.required_scopes)?;
        }
        descriptor = descriptor.with_security(security);

        if let Some(profile_name) = entry.profile.as_deref() {
            if let Some(profile) = self.overrides.profile(profile_name) {
                descriptor = profile.apply(descriptor);
            } else {
                tracing::warn!(
                    module = %module_id,
                    profile = %profile_name,
                    "unknown module service profile"
                );
            }
        }
        descriptor = self.apply_descriptor_overrides(descriptor);

        let route_base = descriptor
            .ingress
            .as_ref()
            .and_then(|ing| ing.route_prefix.clone())
            .unwrap_or_else(|| "/".to_string());
        let endpoint = format!("http://127.0.0.1:{port}{route}", route = route_base);

        Ok((descriptor, endpoint, suffix_owned))
    }

    pub(super) async fn clear_reported_services(&self, module_id: &ModuleId) {
        let removed = {
            let mut guard = self.runtime_services.write().await;
            guard.remove(module_id)
        };
        if let Some(ids) = removed {
            for id in ids {
                self.service_registry.unregister(&id);
            }
        }
        {
            let prefix = format!("service://module:{}::", module_id);
            let mut guard = self.service_endpoints.write().await;
            guard.retain(|uri, _| !uri.starts_with(&prefix));
        }
        self.persist_service_snapshot().await;
    }

    async fn persist_service_snapshot(&self) {
        let Some(path) = &self.service_snapshot_path else {
            return;
        };
        let entries: Vec<ServiceSnapshotEntry> = {
            let guard = self.service_endpoints.read().await;
            guard
                .iter()
                .map(|(uri, endpoint)| ServiceSnapshotEntry {
                    uri: uri.clone(),
                    endpoint: endpoint.clone(),
                })
                .collect()
        };
        let snapshot = ServiceSnapshotFile { services: entries };
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent).await {
                tracing::warn!(error = %err, "failed to ensure service snapshot directory");
                return;
            }
        }
        match serde_json::to_vec_pretty(&snapshot) {
            Ok(data) => {
                if let Err(err) = fs::write(path, data).await {
                    tracing::warn!(error = %err, "failed to persist service snapshot");
                }
            }
            Err(err) => tracing::warn!(error = %err, "failed to serialize service snapshot"),
        }
    }
}

#[derive(Serialize)]
struct ServiceSnapshotFile {
    services: Vec<ServiceSnapshotEntry>,
}

#[derive(Serialize)]
struct ServiceSnapshotEntry {
    uri: String,
    endpoint: String,
}

fn map_kind(value: Option<&str>) -> ServiceKind {
    match value.map(|raw| raw.trim().to_ascii_lowercase()).as_deref() {
        Some("transport") => ServiceKind::Transport,
        Some("infrastructure") => ServiceKind::Infrastructure,
        Some("security") => ServiceKind::Security,
        Some("storage") => ServiceKind::Storage,
        Some("background") | Some("job") => ServiceKind::BackgroundJob,
        Some("cli") => ServiceKind::Cli,
        _ => ServiceKind::Other,
    }
}

fn map_tags(values: &[String]) -> Vec<ServiceTag> {
    values
        .iter()
        .filter_map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "core" => Some(ServiceTag::Core),
            "platform" => Some(ServiceTag::Platform),
            "auxiliary" => Some(ServiceTag::Auxiliary),
            _ => None,
        })
        .collect()
}

fn map_protocols(values: &[String]) -> Vec<ServiceIngressProtocol> {
    let mut protocols = Vec::new();
    for value in values {
        let protocol = match value.trim().to_ascii_lowercase().as_str() {
            "grpc" => ServiceIngressProtocol::Grpc,
            _ => ServiceIngressProtocol::Http,
        };
        if !protocols.contains(&protocol) {
            protocols.push(protocol);
        }
    }
    protocols
}

fn map_roles(values: &[String]) -> Vec<ServiceRole> {
    values
        .iter()
        .filter_map(|value| ServiceRole::from_str(value).ok())
        .collect()
}

fn map_scopes(values: &[String]) -> Result<Vec<ServiceScope>, ModuleRuntimeError> {
    let mut scopes = Vec::new();
    for value in values {
        match ServiceScope::new(value.clone()) {
            Ok(scope) => scopes.push(scope),
            Err(err) => {
                return Err(ModuleRuntimeError::InvalidState(format!(
                    "invalid scope {}: {err}",
                    value
                )))
            }
        }
    }
    Ok(scopes)
}
