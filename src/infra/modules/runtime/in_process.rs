use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimePort, ModuleRuntimeStatus,
    ModuleStartConfig, ModuleStoragePort, ModuleVersion,
};

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
        state.logs.push(format!(
            "[stub] Module {} started in-process",
            state.module_id
        ));
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
            return Ok(ModuleRuntimeInfo {
                module_id: existing.module_id.clone(),
                version: existing.version.clone(),
                status: existing.status.clone(),
                pid: None,
                port: existing.port,
                started_at: existing.started_at,
                stopped_at: existing.stopped_at,
                restart_count: existing.restart_count,
            });
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
            logs: vec![format!(
                "[stub] Module {} v{} prepared",
                installed.manifest.id, version
            )],
            env_keys,
        };
        let start_env_keys = state.env_keys.clone();
        Self::update_state_for_start(&mut state, config.port, start_env_keys);
        let info = ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status: state.status.clone(),
            pid: None,
            port: state.port,
            started_at: state.started_at,
            stopped_at: state.stopped_at,
            restart_count: state.restart_count,
        };
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
            .push(format!("[stub] Module {} stopped", state.module_id));
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

        Ok(ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status: state.status.clone(),
            pid: None,
            port: state.port,
            started_at: state.started_at,
            stopped_at: state.stopped_at,
            restart_count: state.restart_count,
        })
    }

    async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        let state_guard = self.state.read().await;
        Ok(state_guard
            .values()
            .filter(|state| matches!(state.status, ModuleRuntimeStatus::Running))
            .map(|state| ModuleRuntimeInfo {
                module_id: state.module_id.clone(),
                version: state.version.clone(),
                status: state.status.clone(),
                pid: None,
                port: state.port,
                started_at: state.started_at,
                stopped_at: state.stopped_at,
                restart_count: state.restart_count,
            })
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
        state.logs.push(format!(
            "[stub] Module {} restart requested",
            state.module_id
        ));
        Self::update_state_for_start(state, state.port, state.env_keys.clone());
        state
            .logs
            .push(format!("[stub] Module {} restarted", state.module_id));
        Ok(ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status: state.status.clone(),
            pid: None,
            port: state.port,
            started_at: state.started_at,
            stopped_at: state.stopped_at,
            restart_count: state.restart_count,
        })
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
mod tests {
    use super::*;
    use crate::domain::module::{
        InstalledModule, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum,
        ModuleInstallResult, ModuleInstallSource, ModuleManifest, ModuleSignatureDescriptor,
        ModuleStorageError, ModuleStoragePort, SignatureAlgorithm,
    };
    use async_trait::async_trait;
    use std::time::SystemTime;

    struct TestStorage {
        installed: Option<InstalledModule>,
    }

    #[async_trait]
    impl ModuleStoragePort for TestStorage {
        async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
            Ok(self.installed.iter().cloned().collect())
        }

        async fn load(
            &self,
            _id: &ModuleId,
        ) -> Result<Option<InstalledModule>, ModuleStorageError> {
            Ok(self.installed.clone())
        }

        async fn stage_and_activate(
            &self,
            _bundle: ModuleBundle,
            _source: ModuleInstallSource,
        ) -> Result<ModuleInstallResult, ModuleStorageError> {
            Err(ModuleStorageError::InvalidState("not implemented".into()))
        }

        async fn remove(&self, _id: &ModuleId) -> Result<(), ModuleStorageError> {
            Ok(())
        }
    }

    fn test_manifest() -> InstalledModule {
        InstalledModule {
            manifest: ModuleManifest {
                id: "test-module".to_string(),
                version: semver::Version::parse("0.1.0").unwrap(),
                title: None,
                description: None,
                fenrir_version: None,
                authors: vec![],
                license: None,
                artifact: ModuleArtifactDescriptor {
                    download_url: String::new(),
                    checksum: ModuleChecksum {
                        algorithm: crate::domain::module::ChecksumAlgorithm::Sha256,
                        hash: String::new(),
                    },
                    content_type: None,
                    size_bytes: None,
                },
                signature: ModuleSignatureDescriptor {
                    algorithm: SignatureAlgorithm::Ed25519,
                    key_id: String::new(),
                    signature: String::new(),
                },
                tags: vec![],
                published_at: None,
            },
            installed_at: SystemTime::now(),
            path: "test".into(),
            source: ModuleInstallSource::Distribution,
        }
    }

    #[tokio::test]
    async fn start_stop_cycle_tracks_state() {
        let storage = Arc::new(TestStorage {
            installed: Some(test_manifest()),
        });
        let runtime = InProcessModuleRuntime::new(storage);
        let module_id = ModuleId::new("test-module").unwrap();

        let info = runtime
            .start(ModuleStartConfig {
                module_id: module_id.clone(),
                port: Some(8080),
                env_vars: vec![("FOO".to_string(), "bar".to_string())],
                auto_restart: false,
            })
            .await
            .expect("start succeeds");
        assert_eq!(info.port, Some(8080));
        assert!(matches!(info.status, ModuleRuntimeStatus::Running));

        runtime.stop(&module_id).await.unwrap();
        let status = runtime.status(&module_id).await.unwrap();
        assert!(matches!(status.status, ModuleRuntimeStatus::Stopped));

        let logs = runtime.logs(&module_id, None).await.unwrap();
        assert!(logs.iter().any(|line| line.contains("stopped")));
    }

    #[tokio::test]
    async fn restart_increments_counter() {
        let storage = Arc::new(TestStorage {
            installed: Some(test_manifest()),
        });
        let runtime = InProcessModuleRuntime::new(storage);
        let module_id = ModuleId::new("test-module").unwrap();

        runtime
            .start(ModuleStartConfig {
                module_id: module_id.clone(),
                port: None,
                env_vars: vec![],
                auto_restart: false,
            })
            .await
            .unwrap();
        let info = runtime.restart(&module_id).await.unwrap();
        assert_eq!(info.restart_count, 1);
    }
}
