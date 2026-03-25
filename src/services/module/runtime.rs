use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use crate::domain::module::{
    InstalledModule, ModuleId, ModuleManifest, ModuleRuntimeError, ModuleRuntimeInfo,
    ModuleRuntimeKind, ModuleRuntimeStatus, ModuleStartConfig, ModuleVersion,
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
use futures::stream::{self, StreamExt};
use reqwest::StatusCode;
use serde::Serialize;
use tokio::fs;
use tokio::time::sleep;

use super::config::ModuleEnvResolutionError;
use super::reported::{ReportedServiceEntry, ReportedServicesPayload};
use super::service::{MODULE_SERVICE_MANIFEST_PATH, RESERVED_ENV_KEYS};
use super::{
    ModuleClientSettings, ModuleRollingRestartReport, ModuleRuntimeInstanceSnapshot, ModuleService,
    ModuleStartupPhaseReport, ModuleStartupPhaseStatus, ModuleStartupReport, ModuleStartupStatus,
    ModuleStartupTrigger,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const MANIFEST_REFRESH_MAX_ATTEMPTS: u32 = 20;
const MANIFEST_REFRESH_MIN_DELAY_MS: u64 = 500;
const AUTOSTART_CONCURRENCY_DEFAULT: usize = 4;
const AUTOSTART_CONCURRENCY_MIN: usize = 1;
const AUTOSTART_CONCURRENCY_MAX: usize = 16;
const START_PREFLIGHT_TIMEOUT_MS_DEFAULT: u64 = 5_000;
const START_TOKEN_TIMEOUT_MS_DEFAULT: u64 = 5_000;
const START_RUNTIME_TIMEOUT_MS_DEFAULT: u64 = 30_000;
const PREFLIGHT_ARTIFACT_MISSING: &str = "MODULE-PREFLIGHT-001";
const PREFLIGHT_PORT_UNAVAILABLE: &str = "MODULE-PREFLIGHT-002";
const DEPENDENCY_MISSING: &str = "MODULE-DEP-001";
const DEPENDENCY_CYCLE: &str = "MODULE-DEP-002";
const DEPENDENCY_INVALID: &str = "MODULE-DEP-003";
const DEPENDENCY_UPSTREAM_FAILED: &str = "MODULE-DEP-004";
const DEPENDENCY_VERSION_MISMATCH: &str = "MODULE-DEP-005";
const START_PREFLIGHT_TIMEOUT: &str = "MODULE-SLO-001";
const START_TOKEN_TIMEOUT: &str = "MODULE-SLO-002";
const START_RUNTIME_TIMEOUT: &str = "MODULE-SLO-003";

#[derive(Clone)]
struct AutostartNode {
    module_id: ModuleId,
    manifest: ModuleManifest,
    dependencies: Vec<ModuleId>,
}

struct BlockedAutostartModule {
    module_id: ModuleId,
    manifest: ModuleManifest,
    code: &'static str,
    reason: String,
}

struct AutostartPlan {
    layers: Vec<Vec<AutostartNode>>,
    blocked: Vec<BlockedAutostartModule>,
}

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

        let module_count = modules.len();
        let concurrency = self.autostart_concurrency_limit();
        let plan = self.build_autostart_plan(modules);
        tracing::info!(
            event = "module.lifecycle.autostart_started",
            module_count,
            concurrency
        );

        for blocked in plan.blocked {
            let reason = format!("{}: {}", blocked.code, blocked.reason);
            tracing::warn!(
                event = "module.lifecycle.autostart_blocked",
                module_id = %blocked.module_id,
                error_code = blocked.code,
                reason = %reason
            );
            self.update_module_service_status(
                &blocked.module_id,
                &blocked.manifest,
                ServiceStatus::Failed,
                Some(runtime_notes::start_failed(reason.clone())),
            );
            self.store_startup_report(ModuleStartupReport {
                module_id: blocked.module_id,
                trigger: ModuleStartupTrigger::Autostart,
                status: ModuleStartupStatus::Failed,
                started_at: Self::now_rfc3339(),
                completed_at: Self::now_rfc3339(),
                total_duration_ms: 0,
                phases: vec![ModuleStartupPhaseReport {
                    phase: "dependency_resolution".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: 0,
                    error_code: Some(blocked.code.to_string()),
                    message: Some(reason),
                }],
            })
            .await;
        }

        let mut succeeded = HashSet::new();
        for layer in plan.layers {
            let succeeded_snapshot = succeeded.clone();
            let results = stream::iter(layer.into_iter().map(|node| {
                let succeeded_snapshot = succeeded_snapshot.clone();
                async move {
                    if let Some(dep) = node
                        .dependencies
                        .iter()
                        .find(|dep| !succeeded_snapshot.contains(*dep))
                        .cloned()
                    {
                        let reason = format!(
                            "{DEPENDENCY_UPSTREAM_FAILED}: dependency '{dep}' failed to start"
                        );
                        self.update_module_service_status(
                            &node.module_id,
                            &node.manifest,
                            ServiceStatus::Failed,
                            Some(runtime_notes::start_failed(reason.clone())),
                        );
                        self.store_startup_report(ModuleStartupReport {
                            module_id: node.module_id.clone(),
                            trigger: ModuleStartupTrigger::Autostart,
                            status: ModuleStartupStatus::Failed,
                            started_at: Self::now_rfc3339(),
                            completed_at: Self::now_rfc3339(),
                            total_duration_ms: 0,
                            phases: vec![ModuleStartupPhaseReport {
                                phase: "dependency_resolution".to_string(),
                                status: ModuleStartupPhaseStatus::Skipped,
                                duration_ms: 0,
                                error_code: Some(DEPENDENCY_UPSTREAM_FAILED.to_string()),
                                message: Some(reason),
                            }],
                        })
                        .await;
                        return (node.module_id, false);
                    }

                    match self
                        .ensure_running_with_trigger(
                            &node.module_id,
                            ModuleStartupTrigger::Autostart,
                        )
                        .await
                    {
                        Ok(()) => (node.module_id, true),
                        Err(err) => {
                            tracing::warn!(
                                module = %node.module_id,
                                error = %err,
                                "{}",
                                runtime_logs::AUTOSTART_FAILED
                            );
                            (node.module_id, false)
                        }
                    }
                }
            }))
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

            for (module_id, ok) in results {
                if ok {
                    succeeded.insert(module_id);
                }
            }
        }

        tracing::info!(
            event = "module.lifecycle.autostart_completed",
            module_count,
            concurrency
        );
    }

    fn autostart_concurrency_limit(&self) -> usize {
        let raw = env::var("FENRIR_MODULE_AUTOSTART_CONCURRENCY").ok();
        let Some(raw) = raw.as_deref() else {
            return AUTOSTART_CONCURRENCY_DEFAULT;
        };

        match raw.parse::<usize>() {
            Ok(value) => value.clamp(AUTOSTART_CONCURRENCY_MIN, AUTOSTART_CONCURRENCY_MAX),
            Err(err) => {
                tracing::warn!(
                    event = "module.lifecycle.autostart_config_invalid",
                    env_var = "FENRIR_MODULE_AUTOSTART_CONCURRENCY",
                    value = raw,
                    error = %err,
                    fallback = AUTOSTART_CONCURRENCY_DEFAULT
                );
                AUTOSTART_CONCURRENCY_DEFAULT
            }
        }
    }

    fn build_autostart_plan(&self, modules: Vec<InstalledModule>) -> AutostartPlan {
        let mut manifests = HashMap::new();
        for module in modules {
            let Ok(module_id) = module.manifest.module_id() else {
                continue;
            };
            manifests.insert(module_id, module.manifest);
        }

        let mut indegree: HashMap<ModuleId, usize> = HashMap::new();
        let mut outgoing: HashMap<ModuleId, Vec<ModuleId>> = HashMap::new();
        let mut deps_by_module: HashMap<ModuleId, Vec<ModuleId>> = HashMap::new();
        let mut blocked = Vec::new();
        let mut blocked_ids = HashSet::new();

        for module_id in manifests.keys() {
            indegree.insert(module_id.clone(), 0);
            outgoing.insert(module_id.clone(), Vec::new());
            deps_by_module.insert(module_id.clone(), Vec::new());
        }

        for (module_id, manifest) in &manifests {
            let mut seen = HashSet::new();
            for dependency in &manifest.dependencies {
                if !dependency.required() {
                    continue;
                }
                let required_version = match dependency {
                    crate::domain::module::ModuleDependency::Spec(spec) => spec.version.as_ref(),
                    crate::domain::module::ModuleDependency::Id(_) => None,
                };
                let dep_raw = dependency.id().trim();
                let dep_id = match ModuleId::new(dep_raw) {
                    Ok(id) => id,
                    Err(err) => {
                        blocked_ids.insert(module_id.clone());
                        blocked.push(BlockedAutostartModule {
                            module_id: module_id.clone(),
                            manifest: manifest.clone(),
                            code: DEPENDENCY_INVALID,
                            reason: format!("invalid dependency id '{dep_raw}': {err}"),
                        });
                        continue;
                    }
                };

                if dep_id == *module_id {
                    blocked_ids.insert(module_id.clone());
                    blocked.push(BlockedAutostartModule {
                        module_id: module_id.clone(),
                        manifest: manifest.clone(),
                        code: DEPENDENCY_INVALID,
                        reason: "module cannot depend on itself".to_string(),
                    });
                    continue;
                }

                if !seen.insert(dep_id.clone()) {
                    continue;
                }

                if !manifests.contains_key(&dep_id) {
                    blocked_ids.insert(module_id.clone());
                    blocked.push(BlockedAutostartModule {
                        module_id: module_id.clone(),
                        manifest: manifest.clone(),
                        code: DEPENDENCY_MISSING,
                        reason: format!("required dependency '{dep_id}' is not installed"),
                    });
                    continue;
                }

                if let Some(required_version) = required_version {
                    let Some(dep_manifest) = manifests.get(&dep_id) else {
                        continue;
                    };
                    if !required_version.matches(&dep_manifest.version) {
                        blocked_ids.insert(module_id.clone());
                        blocked.push(BlockedAutostartModule {
                            module_id: module_id.clone(),
                            manifest: manifest.clone(),
                            code: DEPENDENCY_VERSION_MISMATCH,
                            reason: format!(
                                "dependency '{dep_id}' does not satisfy version requirement '{}'",
                                required_version
                            ),
                        });
                        continue;
                    }
                }

                if let Some(entry) = indegree.get_mut(module_id) {
                    *entry += 1;
                }
                outgoing
                    .entry(dep_id.clone())
                    .or_default()
                    .push(module_id.clone());
                deps_by_module
                    .entry(module_id.clone())
                    .or_default()
                    .push(dep_id);
            }
        }

        for blocked_id in blocked_ids {
            indegree.remove(&blocked_id);
            outgoing.remove(&blocked_id);
            deps_by_module.remove(&blocked_id);
            for children in outgoing.values_mut() {
                children.retain(|child| child != &blocked_id);
            }
            for deps in deps_by_module.values_mut() {
                deps.retain(|dep| dep != &blocked_id);
            }
        }

        let mut ready = BTreeSet::new();
        for (module_id, degree) in &indegree {
            if *degree == 0 {
                ready.insert(module_id.clone());
            }
        }

        let mut layers = Vec::new();
        let mut processed = HashSet::new();
        while !ready.is_empty() {
            let current: Vec<ModuleId> = ready.iter().cloned().collect();
            for module_id in &current {
                ready.remove(module_id);
            }

            let mut layer = Vec::new();
            for module_id in current {
                processed.insert(module_id.clone());
                let Some(manifest) = manifests.get(&module_id).cloned() else {
                    continue;
                };
                let dependencies = deps_by_module.remove(&module_id).unwrap_or_default();
                layer.push(AutostartNode {
                    module_id: module_id.clone(),
                    manifest,
                    dependencies,
                });

                let children = outgoing.remove(&module_id).unwrap_or_default();
                for child in children {
                    if let Some(degree) = indegree.get_mut(&child) {
                        if *degree > 0 {
                            *degree -= 1;
                            if *degree == 0 {
                                ready.insert(child);
                            }
                        }
                    }
                }
            }
            if !layer.is_empty() {
                layers.push(layer);
            }
        }

        for module_id in indegree.keys() {
            if processed.contains(module_id) {
                continue;
            }
            if let Some(manifest) = manifests.get(module_id) {
                blocked.push(BlockedAutostartModule {
                    module_id: module_id.clone(),
                    manifest: manifest.clone(),
                    code: DEPENDENCY_CYCLE,
                    reason: "dependency cycle detected".to_string(),
                });
            }
        }

        AutostartPlan { layers, blocked }
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
        self.ensure_running_with_trigger(module_id, ModuleStartupTrigger::EnsureRunning)
            .await
    }

    async fn ensure_running_with_trigger(
        &self,
        module_id: &ModuleId,
        trigger: ModuleStartupTrigger,
    ) -> Result<(), ModuleRuntimeError> {
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
                if let Some(port) = info.port {
                    self.port_allocator.record_assigned(module_id, port).await;
                }
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
                self.start_with_trigger(config, trigger).await.map(|_| ())
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
        self.start_with_trigger(config, ModuleStartupTrigger::ManualStart)
            .await
    }

    pub async fn start(
        &self,
        config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        self.start_with_trigger(config, ModuleStartupTrigger::ManualStart)
            .await
    }

    async fn start_with_trigger(
        &self,
        mut config: ModuleStartConfig,
        trigger: ModuleStartupTrigger,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let attempt_started_at = OffsetDateTime::now_utc();
        let attempt_started = Instant::now();
        let mut phases = Vec::new();
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

        let manifest = installed.manifest.clone();
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

        let assigned_port = match config.port {
            Some(port) => Some(port),
            None => self.port_allocator.assigned_port(&config.module_id).await?,
        };
        let preflight_timeout_ms = Self::phase_timeout_ms(
            "FENRIR_MODULE_START_PREFLIGHT_TIMEOUT_MS",
            START_PREFLIGHT_TIMEOUT_MS_DEFAULT,
        );
        let preflight_started = Instant::now();
        let preflight = tokio::time::timeout(
            Duration::from_millis(preflight_timeout_ms),
            self.run_start_preflight(&config.module_id, &installed.path, assigned_port),
        )
        .await;
        let preflight_duration = preflight_started.elapsed().as_millis() as u64;
        match preflight {
            Ok(Ok(())) => phases.push(ModuleStartupPhaseReport {
                phase: "preflight".to_string(),
                status: ModuleStartupPhaseStatus::Succeeded,
                duration_ms: preflight_duration,
                error_code: None,
                message: None,
            }),
            Ok(Err(err)) => {
                let start_reason_prefix = format!("module {} preflight failed", config.module_id);
                let reason = format!("{start_reason_prefix}: {err}");
                phases.push(ModuleStartupPhaseReport {
                    phase: "preflight".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: preflight_duration,
                    error_code: Self::error_code_for_runtime_error(&err),
                    message: Some(err.to_string()),
                });
                tracing::warn!(
                    event = "module.lifecycle.preflight_failed",
                    module_id = %config.module_id,
                    error = %err
                );
                self.update_module_service_status(
                    &config.module_id,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(reason)),
                );
                self.store_startup_report(Self::build_startup_report(
                    &config.module_id,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(err);
            }
            Err(_) => {
                let err = ModuleRuntimeError::StartFailed {
                    module_id: config.module_id.to_string(),
                    reason: format!(
                        "{START_PREFLIGHT_TIMEOUT}: preflight phase exceeded {} ms",
                        preflight_timeout_ms
                    ),
                };
                phases.push(ModuleStartupPhaseReport {
                    phase: "preflight".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: preflight_timeout_ms,
                    error_code: Some(START_PREFLIGHT_TIMEOUT.to_string()),
                    message: Some(err.to_string()),
                });
                self.update_module_service_status(
                    &config.module_id,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(err.to_string())),
                );
                self.store_startup_report(Self::build_startup_report(
                    &config.module_id,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(err);
            }
        }

        tracing::info!(
            event = "module.lifecycle.preflight_passed",
            module_id = %config.module_id,
            assigned_port = ?assigned_port
        );

        let gateway_endpoint = self.ensure_gateway_endpoint(&config.module_id).await?;

        let token_timeout_ms = Self::phase_timeout_ms(
            "FENRIR_MODULE_START_TOKEN_TIMEOUT_MS",
            START_TOKEN_TIMEOUT_MS_DEFAULT,
        );
        let token_started = Instant::now();
        let issued_token = match tokio::time::timeout(
            Duration::from_millis(token_timeout_ms),
            self.issue_module_service_token(&config.module_id),
        )
        .await
        {
            Ok(Ok(token)) => {
                phases.push(ModuleStartupPhaseReport {
                    phase: "service_token".to_string(),
                    status: ModuleStartupPhaseStatus::Succeeded,
                    duration_ms: token_started.elapsed().as_millis() as u64,
                    error_code: None,
                    message: None,
                });
                token
            }
            Ok(Err(err)) => {
                phases.push(ModuleStartupPhaseReport {
                    phase: "service_token".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: token_started.elapsed().as_millis() as u64,
                    error_code: Self::error_code_for_runtime_error(&err),
                    message: Some(err.to_string()),
                });
                self.update_module_service_status(
                    &config.module_id,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(err.to_string())),
                );
                self.stop_gateway_if_any(&config.module_id).await;
                self.store_startup_report(Self::build_startup_report(
                    &config.module_id,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(err);
            }
            Err(_) => {
                let err = ModuleRuntimeError::StartFailed {
                    module_id: config.module_id.to_string(),
                    reason: format!(
                        "{START_TOKEN_TIMEOUT}: token issuance exceeded {} ms",
                        token_timeout_ms
                    ),
                };
                phases.push(ModuleStartupPhaseReport {
                    phase: "service_token".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: token_timeout_ms,
                    error_code: Some(START_TOKEN_TIMEOUT.to_string()),
                    message: Some(err.to_string()),
                });
                self.update_module_service_status(
                    &config.module_id,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(err.to_string())),
                );
                self.stop_gateway_if_any(&config.module_id).await;
                self.store_startup_report(Self::build_startup_report(
                    &config.module_id,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(err);
            }
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

        self.update_module_service_status(
            &config.module_id,
            &manifest,
            ServiceStatus::Starting,
            Some(runtime_notes::STARTING.to_string()),
        );

        let module_id_clone = config.module_id.clone();
        tracing::info!(
            event = "module.lifecycle.boot_started",
            module_id = %module_id_clone,
            assigned_port = ?assigned_port
        );
        let runtime_started_at = Instant::now();
        let runtime_timeout_ms = Self::phase_timeout_ms(
            "FENRIR_MODULE_START_RUNTIME_TIMEOUT_MS",
            START_RUNTIME_TIMEOUT_MS_DEFAULT,
        );
        let runtime_config = config.clone();
        let runtime_result = tokio::time::timeout(
            Duration::from_millis(runtime_timeout_ms),
            self.runtime.start(config),
        )
        .await;
        let runtime_success = matches!(runtime_result, Ok(Ok(_)));
        self.record_lifecycle_metrics(runtime_started_at, runtime_success);
        let runtime_info = match runtime_result {
            Ok(Ok(info)) => {
                phases.push(ModuleStartupPhaseReport {
                    phase: "runtime_start".to_string(),
                    status: ModuleStartupPhaseStatus::Succeeded,
                    duration_ms: runtime_started_at.elapsed().as_millis() as u64,
                    error_code: None,
                    message: None,
                });
                self.clear_health(&info.module_id);
                info
            }
            Ok(Err(err)) => {
                phases.push(ModuleStartupPhaseReport {
                    phase: "runtime_start".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: runtime_started_at.elapsed().as_millis() as u64,
                    error_code: Self::error_code_for_runtime_error(&err),
                    message: Some(err.to_string()),
                });
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
                self.update_module_service_status(
                    &module_id_clone,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(final_err.to_string())),
                );
                tracing::warn!(
                    event = "module.lifecycle.boot_failed",
                    module_id = %module_id_clone,
                    latency_ms = runtime_started_at.elapsed().as_secs_f64() * 1000.0,
                    error = %final_err
                );
                self.store_startup_report(Self::build_startup_report(
                    &module_id_clone,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(final_err);
            }
            Err(_) => {
                let timeout_err = ModuleRuntimeError::StartFailed {
                    module_id: module_id_clone.to_string(),
                    reason: format!(
                        "{START_RUNTIME_TIMEOUT}: runtime start exceeded {} ms",
                        runtime_timeout_ms
                    ),
                };
                phases.push(ModuleStartupPhaseReport {
                    phase: "runtime_start".to_string(),
                    status: ModuleStartupPhaseStatus::Failed,
                    duration_ms: runtime_timeout_ms,
                    error_code: Some(START_RUNTIME_TIMEOUT.to_string()),
                    message: Some(timeout_err.to_string()),
                });
                self.stop_gateway_if_any(&module_id_clone).await;
                let _ = self.runtime.stop(&module_id_clone).await;
                if let Some(token) = issued_token.as_ref() {
                    if let Err(revoke_err) = self
                        .security
                        .revoke_service_token(&token.token, "module-start-timeout")
                    {
                        tracing::warn!(
                            module = %token.claims.actor.identifier(),
                            error = %revoke_err,
                            "{}",
                            module_service_logs::SERVICE_TOKEN_REVOKE_FAILED
                        );
                    }
                }
                self.update_module_service_status(
                    &module_id_clone,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(timeout_err.to_string())),
                );
                tracing::warn!(
                    event = "module.lifecycle.boot_failed",
                    module_id = %module_id_clone,
                    latency_ms = runtime_started_at.elapsed().as_secs_f64() * 1000.0,
                    error = %timeout_err
                );
                self.store_startup_report(Self::build_startup_report(
                    &module_id_clone,
                    trigger,
                    ModuleStartupStatus::Failed,
                    attempt_started_at,
                    attempt_started.elapsed().as_millis() as u64,
                    phases,
                ))
                .await;
                return Err(timeout_err);
            }
        };

        let desired_replicas = self.desired_replica_count(&runtime_info.module_id).max(1);
        if desired_replicas > 1 {
            if let Err(err) = self
                .runtime
                .reconcile_instances(runtime_config.clone(), desired_replicas)
                .await
            {
                self.update_module_service_status(
                    &runtime_info.module_id,
                    &manifest,
                    ServiceStatus::Failed,
                    Some(runtime_notes::start_failed(err.to_string())),
                );
                return Err(err);
            }
        }

        if let Some(token) = issued_token {
            self.audit_runtime_token_refresh(&runtime_info.module_id, &token);
            self.record_service_token(&runtime_info.module_id, &token)
                .await;
        }
        if let Some(port) = runtime_info.port {
            self.port_allocator
                .record_assigned(&runtime_info.module_id, port)
                .await;
        }

        tracing::info!(
            event = "module.lifecycle.boot_succeeded",
            module_id = %runtime_info.module_id,
            pid = ?runtime_info.pid,
            port = ?runtime_info.port,
            latency_ms = runtime_started_at.elapsed().as_secs_f64() * 1000.0,
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

        self.store_startup_report(Self::build_startup_report(
            &runtime_info.module_id,
            trigger,
            ModuleStartupStatus::Succeeded,
            attempt_started_at,
            attempt_started.elapsed().as_millis() as u64,
            phases,
        ))
        .await;

        Ok(runtime_info)
    }

    fn phase_timeout_ms(env_key: &str, default: u64) -> u64 {
        let raw = match env::var(env_key) {
            Ok(value) => value,
            Err(_) => return default,
        };
        match raw.trim().parse::<u64>() {
            Ok(value) if value > 0 => value,
            Ok(_) | Err(_) => {
                tracing::warn!(
                    event = "module.lifecycle.start_timeout_config_invalid",
                    env_var = env_key,
                    value = raw,
                    fallback = default
                );
                default
            }
        }
    }

    async fn store_startup_report(&self, report: ModuleStartupReport) {
        self.startup_reports
            .write()
            .await
            .insert(report.module_id.clone(), report);
    }

    fn build_startup_report(
        module_id: &ModuleId,
        trigger: ModuleStartupTrigger,
        status: ModuleStartupStatus,
        started_at: OffsetDateTime,
        total_duration_ms: u64,
        phases: Vec<ModuleStartupPhaseReport>,
    ) -> ModuleStartupReport {
        ModuleStartupReport {
            module_id: module_id.clone(),
            trigger,
            status,
            started_at: Self::format_offset_rfc3339(started_at),
            completed_at: Self::format_offset_rfc3339(OffsetDateTime::now_utc()),
            total_duration_ms,
            phases,
        }
    }

    fn error_code_for_runtime_error(err: &ModuleRuntimeError) -> Option<String> {
        match err {
            ModuleRuntimeError::StartFailed { reason, .. } => {
                let candidate = reason.split(':').next()?.trim();
                if candidate.is_empty() {
                    return None;
                }
                if candidate
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-')
                {
                    Some(candidate.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn now_rfc3339() -> String {
        Self::format_offset_rfc3339(OffsetDateTime::now_utc())
    }

    fn format_offset_rfc3339(value: OffsetDateTime) -> String {
        value
            .format(&Rfc3339)
            .unwrap_or_else(|_| value.unix_timestamp().to_string())
    }

    pub async fn startup_reports(&self) -> Vec<ModuleStartupReport> {
        let guard = self.startup_reports.read().await;
        let mut reports: Vec<_> = guard.values().cloned().collect();
        reports.sort_by(|a, b| a.module_id.cmp(&b.module_id));
        reports
    }

    pub async fn startup_report(&self, module_id: &ModuleId) -> Option<ModuleStartupReport> {
        self.startup_reports.read().await.get(module_id).cloned()
    }

    async fn run_start_preflight(
        &self,
        module_id: &ModuleId,
        installed_path: &str,
        assigned_port: Option<u16>,
    ) -> Result<(), ModuleRuntimeError> {
        tracing::info!(
            event = "module.lifecycle.preflight_started",
            module_id = %module_id,
            assigned_port = ?assigned_port
        );

        if fs::metadata(Path::new(installed_path)).await.is_err() {
            return Err(ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: format!(
                    "{PREFLIGHT_ARTIFACT_MISSING}: installed artifact path missing ({installed_path})"
                ),
            });
        }

        if let Some(port) = assigned_port {
            let bind_addr = format!("127.0.0.1:{port}");
            match tokio::net::TcpListener::bind(&bind_addr).await {
                Ok(listener) => drop(listener),
                Err(err) => {
                    return Err(ModuleRuntimeError::StartFailed {
                        module_id: module_id.to_string(),
                        reason: format!(
                            "{PREFLIGHT_PORT_UNAVAILABLE}: failed to reserve {bind_addr} ({err})"
                        ),
                    });
                }
            }
        }

        Ok(())
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
        self.diagnostics
            .clear_runtime_metrics(&Self::module_service_id(module_id));
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

    pub async fn runtime_instances(
        &self,
        module_id: &ModuleId,
    ) -> Result<Vec<ModuleRuntimeInstanceSnapshot>, ModuleRuntimeError> {
        let infos = self.runtime.list_instances(module_id).await?;
        Ok(infos
            .into_iter()
            .map(ModuleRuntimeInstanceSnapshot::from_instance_info)
            .collect())
    }

    pub async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        self.runtime.list_running().await
    }

    pub async fn list_runtime_instances(
        &self,
    ) -> Result<Vec<ModuleRuntimeInstanceSnapshot>, ModuleRuntimeError> {
        let infos = self.runtime.list_running().await?;
        Ok(infos
            .into_iter()
            .map(ModuleRuntimeInstanceSnapshot::from_runtime_info)
            .collect())
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

        self.update_module_service_status(
            module_id,
            &installed.manifest,
            ServiceStatus::Standby,
            Some("draining for controlled restart".to_string()),
        );
        sleep(self.rollout_settings.drain_before_restart).await;

        self.stop(module_id).await?;

        let mut info = self
            .start_with_trigger(
                ModuleStartConfig {
                    module_id: module_id.clone(),
                    port: None,
                    env_vars: vec![],
                    auto_restart: false,
                },
                ModuleStartupTrigger::Restart,
            )
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

    pub async fn rolling_restart(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRollingRestartReport, ModuleRuntimeError> {
        let installed = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id.to_string(),
            })?;
        let snapshots = self.runtime_instances(module_id).await?;
        if snapshots.is_empty() {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id.to_string(),
            });
        }

        let mut restarted_instances = Vec::new();
        let mut health_verified = true;
        let desired_instances = snapshots.len();
        let env_vars = self.runtime.env(module_id).await.unwrap_or_default();
        for (index, snapshot) in snapshots.iter().enumerate() {
            let info = if desired_instances > 1 {
                self.surge_replace_instance(
                    module_id,
                    &installed.manifest,
                    snapshot,
                    desired_instances,
                    env_vars.clone(),
                )
                .await?
            } else {
                if snapshot.port.is_some() {
                    self.update_instance_health(
                        module_id,
                        &snapshot.instance_id,
                        false,
                        Some("single-instance restart in progress".to_string()),
                    );
                }
                let info = self
                    .runtime
                    .restart_instance(module_id, &snapshot.instance_id)
                    .await?;
                let _ = self
                    .refresh_reported_services(module_id, &installed.manifest, info.port)
                    .await;
                info
            };
            let health_result = self
                .verify_module_rollout_health(module_id, info.port)
                .await;
            if health_result.is_err() {
                health_verified = false;
            }
            restarted_instances.push(ModuleRuntimeInstanceSnapshot {
                instance_id: snapshot.instance_id.clone(),
                primary: snapshot.primary,
                ..ModuleRuntimeInstanceSnapshot::from_runtime_info(info)
            });
            if index + 1 < snapshots.len() && !self.rollout_settings.inter_restart_delay.is_zero() {
                sleep(self.rollout_settings.inter_restart_delay).await;
            }
            if !health_verified && self.rollout_settings.abort_on_first_failure {
                break;
            }
        }

        let note = if snapshots.len() == 1 {
            "single-instance restart executed; zero-downtime replacement requires replicas >= 2"
                .to_string()
        } else if health_verified {
            format!(
                "rolling replace completed across {} instances without dropping replica capacity",
                restarted_instances.len()
            )
        } else {
            format!(
                "rolling replace aborted after {} instances because a health check failed",
                restarted_instances.len()
            )
        };

        Ok(ModuleRollingRestartReport {
            module_id: module_id.clone(),
            restarted_instances,
            health_verified,
            note,
        })
    }

    async fn surge_replace_instance(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        target: &ModuleRuntimeInstanceSnapshot,
        desired_instances: usize,
        env_vars: Vec<(String, String)>,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let before = self.runtime.list_instances(module_id).await?;
        let before_ids = before
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect::<HashSet<_>>();
        let surged = self
            .runtime
            .reconcile_instances(
                ModuleStartConfig {
                    module_id: module_id.clone(),
                    port: None,
                    env_vars: env_vars.clone(),
                    auto_restart: false,
                },
                desired_instances.saturating_add(1),
            )
            .await?;

        let surge_instance = surged
            .iter()
            .find(|instance| !before_ids.contains(&instance.instance_id))
            .cloned()
            .ok_or_else(|| {
                ModuleRuntimeError::InvalidState(format!(
                    "failed to identify surge instance for module {module_id}"
                ))
            })?;

        let surge_health = self
            .verify_module_rollout_health(module_id, surge_instance.runtime.port)
            .await;
        if surge_health.is_err() {
            if surge_instance.runtime.port.is_some() {
                self.update_instance_health(
                    module_id,
                    &surge_instance.instance_id,
                    false,
                    Some("surge instance failed readiness".to_string()),
                );
            }
            let _ = self
                .runtime
                .reconcile_instances(
                    ModuleStartConfig {
                        module_id: module_id.clone(),
                        port: None,
                        env_vars,
                        auto_restart: false,
                    },
                    desired_instances,
                )
                .await;
            return Err(ModuleRuntimeError::InvalidState(
                surge_health.unwrap_or_else(|err| err),
            ));
        }
        if surge_instance.runtime.port.is_some() {
            self.update_instance_health(module_id, &surge_instance.instance_id, true, None);
        }

        if target.port.is_some() {
            self.update_instance_health(
                module_id,
                &target.instance_id,
                false,
                Some("instance draining for surge replacement".to_string()),
            );
        }

        let restarted = self
            .runtime
            .restart_instance(module_id, &target.instance_id)
            .await?;
        let _ = self
            .refresh_reported_services(module_id, manifest, restarted.port)
            .await;

        let restart_health = self
            .verify_module_rollout_health(module_id, restarted.port)
            .await;
        if restart_health.is_err() {
            if restarted.port.is_some() {
                self.update_instance_health(
                    module_id,
                    &target.instance_id,
                    false,
                    Some("restarted instance failed readiness".to_string()),
                );
            }
            return Err(ModuleRuntimeError::InvalidState(
                restart_health.unwrap_or_else(|err| err),
            ));
        }
        if restarted.port.is_some() {
            self.update_instance_health(module_id, &target.instance_id, true, None);
        }

        let _ = self
            .runtime
            .reconcile_instances(
                ModuleStartConfig {
                    module_id: module_id.clone(),
                    port: None,
                    env_vars,
                    auto_restart: false,
                },
                desired_instances,
            )
            .await?;
        let _ = self
            .refresh_reported_services(module_id, manifest, restarted.port)
            .await;

        Ok(restarted)
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
            .apply_reported_services(module_id, &installed.manifest, vec![(port, payload)])
            .await?;
        Ok(())
    }

    pub async fn runtime_env(
        &self,
        module_id: &ModuleId,
    ) -> Result<Vec<(String, String)>, ModuleRuntimeError> {
        self.runtime.env(module_id).await
    }

    fn desired_replica_count(&self, module_id: &ModuleId) -> usize {
        let service_id = Self::module_service_id(module_id);
        self.with_overrides(|config| config.desired_replicas_for(&service_id))
            .unwrap_or(1)
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
        self.append_env_passthrough(&mut env, module_id);
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
        let overrides = self.with_overrides(|config| config.env_for(service_id));
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

    fn append_env_passthrough(&self, env: &mut Vec<(String, String)>, module_id: &ModuleId) {
        let mut prefixes: Vec<String> = self
            .env_passthrough_prefixes
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        prefixes.extend(Self::derived_env_prefixes(module_id));
        if prefixes.is_empty() {
            return;
        }

        let mut seen = HashSet::new();
        prefixes.retain(|prefix| seen.insert(prefix.to_ascii_uppercase()));

        for (key, value) in env::vars() {
            if key.starts_with("FENRIR_") || RESERVED_ENV_KEYS.contains(&key.as_str()) {
                continue;
            }
            if !prefixes.iter().any(|prefix| key.starts_with(prefix)) {
                continue;
            }
            if env.iter().any(|(existing, _)| existing == &key) {
                continue;
            }
            env.push((key, value));
        }
    }

    fn derived_env_prefixes(module_id: &ModuleId) -> Vec<String> {
        let normalized: String = module_id
            .to_string()
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        let normalized = normalized.trim_matches('_').to_string();
        if normalized.is_empty() {
            return Vec::new();
        }

        let mut prefixes = vec![format!("{normalized}_")];
        if let Some(first_segment) = normalized.split('_').find(|segment| !segment.is_empty()) {
            let short = format!("{first_segment}_");
            if short != prefixes[0] {
                prefixes.push(short);
            }
        }
        prefixes
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
        preferred_port: Option<u16>,
    ) -> Result<bool, ModuleRuntimeError> {
        let mut ports = self
            .runtime
            .list_instances(module_id)
            .await?
            .into_iter()
            .filter(|instance| {
                matches!(instance.runtime.status, ModuleRuntimeStatus::Running)
                    && instance.runtime.port.is_some()
            })
            .filter_map(|instance| instance.runtime.port)
            .collect::<Vec<_>>();
        if let Some(preferred_port) = preferred_port {
            ports.sort_by_key(|port| if *port == preferred_port { 0 } else { 1 });
            ports.dedup();
        }
        if ports.is_empty() {
            self.clear_reported_services(module_id).await;
            return Ok(false);
        }

        let attempt_count = self.client_settings.retries.saturating_add(1).max(2);
        let mut attempt = 0u32;
        let mut last_error: Option<String> = None;

        while attempt < attempt_count {
            let mut manifests = Vec::new();
            for port in &ports {
                match self.fetch_reported_services(module_id, *port).await {
                    Ok(Some(payload)) => manifests.push((*port, payload)),
                    Ok(None) => {}
                    Err(err) => {
                        last_error = Some(err.to_string());
                    }
                }
            }

            if self
                .apply_reported_services(module_id, manifest, manifests)
                .await?
            {
                return Ok(true);
            }

            if attempt + 1 < attempt_count {
                tracing::debug!(
                    module = %module_id,
                    attempt = attempt + 1,
                    "module service manifest not yet reachable on any running instance, retrying"
                );
                sleep(self.client_settings.backoff).await;
                attempt += 1;
                continue;
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
        payloads: Vec<(u16, ReportedServicesPayload)>,
    ) -> Result<bool, ModuleRuntimeError> {
        let payloads = payloads
            .into_iter()
            .filter(|(_, payload)| !payload.services.is_empty())
            .collect::<Vec<_>>();
        if payloads.is_empty() {
            self.clear_reported_services(module_id).await;
            return Ok(false);
        }

        self.clear_reported_services(module_id).await;

        let mut registered = Vec::new();
        let mut endpoints: HashMap<String, Vec<String>> = HashMap::new();
        let (primary_port, primary_payload) = &payloads[0];

        for entry in &primary_payload.services {
            match self
                .build_descriptor_from_report(module_id, manifest, *primary_port, entry.clone())
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
                    endpoints.insert(
                        Self::module_runtime_service_uri(module_id, &suffix),
                        vec![endpoint],
                    );
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

        for (port, payload) in payloads.iter().skip(1) {
            for entry in &payload.services {
                if entry.service_id.trim().is_empty() {
                    continue;
                }
                let uri = Self::module_runtime_service_uri(module_id, entry.service_id.trim());
                let endpoint =
                    Self::reported_service_endpoint(*port, entry.route_prefix.as_deref());
                let entry_endpoints = endpoints.entry(uri).or_default();
                if !entry_endpoints.iter().any(|existing| existing == &endpoint) {
                    entry_endpoints.push(endpoint);
                }
            }
        }

        {
            let mut guard = self.runtime_services.write().await;
            guard.insert(module_id.clone(), registered);
        }

        {
            let prefix = format!("service://module:{}::", module_id);
            let mut guard = self.service_endpoints.write().await;
            guard.retain(|uri, _| !uri.starts_with(&prefix));
            for (uri, mut endpoint_list) in endpoints {
                endpoint_list.sort();
                guard.insert(uri, endpoint_list);
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
            if let Some(profile) = self.with_overrides(|overrides| overrides.profile(profile_name))
            {
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
        let endpoint = Self::reported_service_endpoint(port, Some(&route_base));

        Ok((descriptor, endpoint, suffix_owned))
    }

    fn reported_service_endpoint(port: u16, route_prefix: Option<&str>) -> String {
        let route = route_prefix
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("/");
        format!("http://127.0.0.1:{port}{route}")
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
                .map(|(uri, endpoints)| ServiceSnapshotEntry {
                    uri: uri.clone(),
                    endpoint: endpoints.first().cloned().unwrap_or_default(),
                    endpoints: endpoints.clone(),
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
    endpoints: Vec<String>,
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
