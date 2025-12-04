use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::fs;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::dev_agent::config::{DevAgentConfig, DevAgentService};
use crate::domain::module::{ModuleId, ModuleResult, ModuleServiceError, ModuleStorageError};
use crate::services::module::types::{ModuleDevRunState, ModuleDevServices, RegisteredDevService};
use crate::utils::messages::services::module::dev::{
    errors as module_dev_errors, logs as module_dev_logs,
};
use time::OffsetDateTime;

use super::dev::{DevRunCommand, DevRunConfig};
use super::service::{ModuleService, RESERVED_ENV_KEYS};

pub(super) struct DevAgentHandle {
    pub child: Child,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
    pub workdir: PathBuf,
    pub run: Arc<DevRunConfig>,
    pub services: Arc<Vec<RegisteredDevService>>,
}

impl ModuleService {
    pub(super) async fn start_dev_agent(
        &self,
        module_id: &ModuleId,
        dev_services: &ModuleDevServices,
        run: DevRunConfig,
    ) -> ModuleResult<(ModuleDevRunState, OffsetDateTime)> {
        let module_root = match self.dev_module_root(module_id) {
            Some(path) => path,
            None => {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(module_dev_errors::dev_root_not_directory(
                        "dev sources",
                        module_id,
                    )),
                ));
            }
        };
        if dev_services.services.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::no_dev_services(module_id)),
            ));
        }
        let run_arc = Arc::new(run.clone());
        let DevRunConfig {
            command,
            workdir,
            auto_restart,
            env: extra_env,
            ..
        } = run;
        let DevRunCommand { args, display } = command;

        let resolved_workdir = workdir.unwrap_or_else(|| module_root.clone());
        if !resolved_workdir.exists() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_run_workdir_missing(
                    module_root.display(),
                    resolved_workdir.display(),
                )),
            ));
        }

        let primary_endpoint = dev_services.services.first().map(|svc| svc.endpoint);
        let env_export = self
            .export_runtime_environment(module_id, primary_endpoint)
            .await?;
        let mut env_map: HashMap<String, String> = env_export.entries.into_iter().collect();
        for (key, value) in extra_env {
            if RESERVED_ENV_KEYS
                .iter()
                .any(|reserved| reserved == &key.as_str())
            {
                warn!(
                    module = %module_id,
                    key = %key,
                    "ignoring attempt to override reserved env key"
                );
                continue;
            }
            env_map.insert(key, value);
        }

        let agent_dir = module_root.join(".fenrir").join("dev-agent");
        let log_path = agent_dir.join("run.log");
        let config_path = agent_dir.join("config.json");
        fs::create_dir_all(&agent_dir)
            .await
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))?;

        let services_arc = Arc::new(dev_services.services.clone());
        let services = services_arc
            .iter()
            .map(|svc| DevAgentService {
                id: svc.service_id.clone(),
                endpoint: svc.endpoint.to_string(),
            })
            .collect::<Vec<_>>();

        let config = DevAgentConfig {
            module_id: module_id.to_string(),
            command: args,
            command_display: display.clone(),
            workdir: resolved_workdir.clone(),
            auto_restart,
            env: env_map,
            log_path: log_path.clone(),
            services,
        };

        Self::write_agent_config(&config_path, &config).await?;

        info!(
            module = %module_id,
            command = %config.command_display,
            workdir = %resolved_workdir.display(),
            "{}",
            module_dev_logs::DEV_AGENT_STARTING
        );

        let child = match self.spawn_dev_agent_process(module_id, &config_path) {
            Ok(child) => child,
            Err(err) => {
                let _ = fs::remove_file(&config_path).await;
                return Err(err);
            }
        };

        let handle = Arc::new(Mutex::new(DevAgentHandle {
            child,
            config_path: config_path.clone(),
            log_path: log_path.clone(),
            workdir: resolved_workdir.clone(),
            run: Arc::clone(&run_arc),
            services: Arc::clone(&services_arc),
        }));
        {
            let mut guard = self.dev_agents.lock().await;
            guard.insert(module_id.clone(), handle);
        }

        Ok((
            ModuleDevRunState {
                command: display,
                workdir: resolved_workdir,
                auto_restart,
                auto_start: true,
                log_path: Some(log_path),
            },
            env_export.token.claims.expires_at,
        ))
    }

    pub(super) async fn stop_dev_agent_if_any(&self, module_id: &ModuleId) {
        let handle_arc = {
            let mut guard = self.dev_agents.lock().await;
            guard.remove(module_id)
        };
        if let Some(handle_arc) = handle_arc {
            self.cancel_dev_agent_rotation(module_id).await;
            let mut handle = handle_arc.lock().await;
            info!(module = %module_id, "{}", module_dev_logs::DEV_AGENT_STOPPING);
            if let Err(err) = handle.child.start_kill() {
                warn!(module = %module_id, error = %err, "failed to stop dev agent");
            } else if let Err(err) = handle.child.wait().await {
                warn!(module = %module_id, error = %err, "dev agent wait failed");
            }
            let _ = fs::remove_file(&handle.config_path).await;
        }
    }

    pub(super) async fn write_agent_config(
        path: &PathBuf,
        config: &DevAgentConfig,
    ) -> ModuleResult<()> {
        let serialized = serde_json::to_vec_pretty(config).map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        fs::write(path, serialized)
            .await
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))?;
        Ok(())
    }

    pub(super) fn spawn_dev_agent_process(
        &self,
        module_id: &ModuleId,
        config_path: &Path,
    ) -> Result<Child, ModuleServiceError> {
        let binary = self.resolve_agent_binary()?;
        let mut command = Command::new(binary);
        command
            .arg("--dev-agent-config")
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_dev_errors::dev_agent_spawn_failed(module_id, err),
            ))
        })
    }

    fn resolve_agent_binary(&self) -> Result<PathBuf, ModuleServiceError> {
        match std::env::current_exe() {
            Ok(path) if path.exists() => Ok(path),
            Ok(path) => Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_agent_binary_missing(
                    path.display(),
                )),
            )),
            Err(err) => Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(err.to_string()),
            )),
        }
    }
}
