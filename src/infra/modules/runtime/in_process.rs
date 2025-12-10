use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimePort, ModuleRuntimeStatus,
    ModuleStartConfig, ModuleStoragePort, ModuleVersion,
};
use crate::utils::messages;

/// In-process runtime stub that simulates module lifecycle without spawning OS processes.
///
/// Intended for testing and local development scenarios. State is kept in-memory and
/// validated against the configured module storage to ensure only installed modules
/// can be started.
pub struct InProcessModuleRuntime {
    storage: Arc<dyn ModuleStoragePort>,
    state: Arc<RwLock<HashMap<String, ModuleRuntimeState>>>,
}

#[derive(Debug, Clone)]
struct ModuleRuntimeState {
    module_id: ModuleId,
    version: ModuleVersion,
    port: Option<u16>,
    started_at: Option<SystemTime>,
    stopped_at: Option<SystemTime>,
    restart_count: u32,
    status: ModuleRuntimeStatus,
    logs: Vec<String>,
    env_keys: Vec<String>,
}

impl InProcessModuleRuntime {
    pub fn new(storage: Arc<dyn ModuleStoragePort>) -> Self {
        Self {
            storage,
            state: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn now() -> SystemTime {
        SystemTime::now()
    }

    fn update_state_for_start(
        state: &mut ModuleRuntimeState,
        port: Option<u16>,
        env_keys: Vec<String>,
    ) {
        state.port = port;
        state.started_at = Some(Self::now());
        state.stopped_at = None;
        state.status = ModuleRuntimeStatus::Running;
        state.env_keys = env_keys;
        state
            .logs
            .push(messages::infra::modules::runtime::in_process::started(
                state.module_id.clone(),
            ));
    }

    fn info_from_state(state: &ModuleRuntimeState) -> ModuleRuntimeInfo {
        ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status: state.status.clone(),
            pid: None,
            port: state.port,
            started_at: state.started_at,
            stopped_at: state.stopped_at,
            restart_count: state.restart_count,
        }
    }
}

#[async_trait]
impl ModuleRuntimePort for InProcessModuleRuntime {
    async fn start(
        &self,
        mut config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = config.module_id.to_string();

        let mut state_guard = self.state.write().await;
        if let Some(existing) = state_guard.get_mut(&module_id_str) {
            if matches!(existing.status, ModuleRuntimeStatus::Running) {
                return Err(ModuleRuntimeError::AlreadyRunning {
                    module_id: module_id_str,
                });
            }
            let env_keys: Vec<String> = config.env_vars.drain(..).map(|(k, _)| k).collect();
            Self::update_state_for_start(existing, config.port, env_keys.clone());
            return Ok(Self::info_from_state(existing));
        }

        let installed = self
            .storage
            .load(&config.module_id)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id_str.clone(),
            })?;

        let version = ModuleVersion(installed.manifest.version.clone());
        let env_keys: Vec<String> = config.env_vars.drain(..).map(|(k, _)| k).collect();
        let mut state = ModuleRuntimeState {
            module_id: config.module_id.clone(),
            version: version.clone(),
            port: None,
            started_at: None,
            stopped_at: None,
            restart_count: 0,
            status: ModuleRuntimeStatus::Starting,
            logs: vec![messages::infra::modules::runtime::in_process::prepared(
                &installed.manifest.id,
                &version,
            )],
            env_keys,
        };
        let start_env_keys = state.env_keys.clone();
        Self::update_state_for_start(&mut state, config.port, start_env_keys);
        let info = Self::info_from_state(&state);
        state_guard.insert(module_id_str, state);
        Ok(info)
    }

    async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let mut state_guard = self.state.write().await;
        let state =
            state_guard
                .get_mut(&module_id_str)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                })?;

        if !matches!(state.status, ModuleRuntimeStatus::Running) {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id_str,
            });
        }

        state.status = ModuleRuntimeStatus::Stopped;
        state.stopped_at = Some(Self::now());
        state
            .logs
            .push(messages::infra::modules::runtime::in_process::stopped(
                state.module_id.clone(),
            ));
        Ok(())
    }

    async fn status(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let state_guard = self.state.read().await;
        let state =
            state_guard
                .get(&module_id_str)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                })?;

        Ok(Self::info_from_state(state))
    }

    async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        let state_guard = self.state.read().await;
        Ok(state_guard
            .values()
            .filter(|state| matches!(state.status, ModuleRuntimeStatus::Running))
            .map(Self::info_from_state)
            .collect())
    }

    async fn restart(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let mut state_guard = self.state.write().await;
        let state =
            state_guard
                .get_mut(&module_id_str)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                })?;

        if !matches!(state.status, ModuleRuntimeStatus::Running) {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id_str,
            });
        }

        state.restart_count = state.restart_count.saturating_add(1);
        state.logs.push(
            messages::infra::modules::runtime::in_process::restart_requested(
                state.module_id.clone(),
            ),
        );
        Self::update_state_for_start(state, state.port, state.env_keys.clone());
        state
            .logs
            .push(messages::infra::modules::runtime::in_process::restarted(
                state.module_id.clone(),
            ));
        Ok(Self::info_from_state(state))
    }

    async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let state_guard = self.state.read().await;
        let state =
            state_guard
                .get(&module_id_str)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                })?;

        let logs = if let Some(limit) = tail {
            state.logs.iter().rev().take(limit).rev().cloned().collect()
        } else {
            state.logs.clone()
        };

        Ok(logs)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/infra/modules/runtime/in_process_tests.rs"]
mod tests;
