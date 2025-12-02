use std::time::SystemTime;

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimeStatus, ModuleStartConfig,
    ModuleVersion,
};
use crate::services::ServiceStatus;
use crate::utils::messages::services::module::runtime::{
    logs as runtime_logs, notes as runtime_notes,
};

use super::ModuleService;

impl ModuleService {
    pub async fn ensure_all_running(&self) {
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
            if let Err(err) = self.runtime.stop(&info.module_id).await {
                if !matches!(err, ModuleRuntimeError::NotRunning { .. }) {
                    tracing::warn!(
                        module = %info.module_id,
                        error = %err,
                        "{}",
                        runtime_logs::STOP_DURING_SYNC_FAILED
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) async fn stop_module_process(&self, module_id: &ModuleId) {
        match self.runtime.stop(module_id).await {
            Ok(_) => {}
            Err(ModuleRuntimeError::NotRunning { .. }) => {}
            Err(err) => {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    runtime_logs::STOP_BEFORE_UPDATE_FAILED
                );
            }
        }
    }

    pub async fn ensure_running(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
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

        self.register_declared_services(module_id, &installed)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        match self.runtime.status(module_id).await {
            Ok(info) if matches!(info.status, ModuleRuntimeStatus::Running) => {
                self.update_module_service_status(
                    module_id,
                    &installed.manifest,
                    ServiceStatus::Active,
                    Some(
                        info.pid
                            .map(runtime_notes::running_with_pid)
                            .unwrap_or_else(|| runtime_notes::RUNNING.to_string()),
                    ),
                );
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

        let assigned_port = match config.port {
            Some(port) => Some(port),
            None => self.port_allocator.assigned_port(&config.module_id).await?,
        };
        config.port = assigned_port;
        config.env_vars =
            Self::inject_runtime_env(config.env_vars, &config.module_id, assigned_port);

        let manifest = installed.manifest.clone();
        let runtime_info = self.runtime.start(config).await?;

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
            Some(
                runtime_info
                    .pid
                    .map(runtime_notes::running_with_pid)
                    .unwrap_or_else(|| runtime_notes::RUNNING.to_string()),
            ),
        );

        Ok(runtime_info)
    }

    pub async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        if self.is_unmanaged_module(module_id) || self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        self.runtime.stop(module_id).await?;

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
            Some(
                info.pid
                    .map(runtime_notes::running_with_pid)
                    .unwrap_or_else(|| runtime_notes::RUNNING.to_string()),
            ),
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

    fn inject_runtime_env(
        mut env: Vec<(String, String)>,
        module_id: &ModuleId,
        port: Option<u16>,
    ) -> Vec<(String, String)> {
        const RESERVED_KEYS: [&str; 5] = [
            "FENRIR_MODULE_ID",
            "FENRIR_SERVICE_ID",
            "FENRIR_SERVICE_URI",
            "FENRIR_SERVICE_PORT",
            "FENRIR_SERVICE_ADDR",
        ];
        env.retain(|(key, _)| {
            !RESERVED_KEYS
                .iter()
                .any(|reserved| *reserved == key.as_str())
        });

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
        env
    }
}
