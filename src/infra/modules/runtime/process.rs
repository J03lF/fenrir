use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimePort, ModuleRuntimeStatus,
    ModuleStartConfig, ModuleStoragePort, ModuleVersion,
};

/// State file name for persisting runtime information
const STATE_FILE_NAME: &str = "runtime-state.json";

/// Minimal config structure to extract port information
#[derive(Debug, Deserialize)]
struct ModuleConfigPort {
    #[serde(default)]
    server: Option<ServerConfigPort>,
}

#[derive(Debug, Deserialize)]
struct ServerConfigPort {
    port: Option<u16>,
}

/// Process-based module runtime implementation
pub struct ProcessModuleRuntime {
    storage: Arc<dyn ModuleStoragePort>,
    state_dir: PathBuf,
    running_modules: Arc<RwLock<HashMap<String, RunningModuleState>>>,
}

#[derive(Debug)]
struct RunningModuleState {
    module_id: ModuleId,
    version: ModuleVersion,
    pid: u32,
    port: Option<u16>,
    started_at: SystemTime,
    restart_count: u32,
    log_file: PathBuf,
    #[allow(dead_code)]
    child: Option<Child>,
}

impl ProcessModuleRuntime {
    pub fn new(storage: Arc<dyn ModuleStoragePort>, state_dir: PathBuf) -> Self {
        if let Err(err) = std::fs::create_dir_all(&state_dir) {
            warn!(
                path = %state_dir.display(),
                error = %err,
                "failed to prepare module runtime state directory"
            );
        }
        Self {
            storage,
            state_dir,
            running_modules: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load persisted runtime state on startup
    pub async fn load_state(&self) -> Result<(), ModuleRuntimeError> {
        let state_file = self.state_dir.join(STATE_FILE_NAME);

        if !state_file.exists() {
            debug!("no runtime state file found, starting fresh");
            return Ok(());
        }

        match fs::read_to_string(&state_file).await {
            Ok(contents) => {
                match serde_json::from_str::<Vec<PersistedModuleState>>(&contents) {
                    Ok(states) => {
                        info!("loaded {} module states from disk", states.len());

                        // Check which modules are still running
                        for state in states {
                            if self.is_process_alive(state.pid) {
                                if let Err(err) = self.reattach_running_module(state).await {
                                    warn!(error = %err, "failed to reattach running module");
                                }
                            } else {
                                warn!(
                                    module_id = %state.module_id,
                                    pid = state.pid,
                                    "module was running but process no longer exists"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!("failed to parse runtime state file: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("failed to read runtime state file: {}", e);
            }
        }

        Ok(())
    }

    async fn reattach_running_module(
        &self,
        persisted: PersistedModuleState,
    ) -> Result<(), ModuleRuntimeError> {
        let module_id = ModuleId::new(&persisted.module_id)
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;
        let expected_version = ModuleVersion::parse(&persisted.version)
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        let installed = self
            .storage
            .load(&module_id)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if let Some(installed) = installed {
            let installed_version = ModuleVersion(installed.manifest.version.clone());
            if installed_version != expected_version {
                warn!(
                    module_id = %module_id,
                    expected = %expected_version,
                    actual = %installed_version,
                    "reattach version mismatch"
                );
            }
        } else {
            warn!(
                module_id = %module_id,
                "installed module missing during runtime reattach"
            );
            return Ok(());
        }

        let started_at = UNIX_EPOCH + Duration::from_secs(persisted.started_at);
        let state = RunningModuleState {
            module_id: module_id.clone(),
            version: expected_version.clone(),
            pid: persisted.pid,
            port: persisted.port,
            started_at,
            restart_count: persisted.restart_count,
            log_file: PathBuf::from(&persisted.log_file),
            child: None,
        };

        let key = module_id.to_string();
        {
            let mut modules = self.running_modules.write().await;
            modules.insert(key, state);
        }

        crate::infra::telemetry::register_service_process("module-runtime", persisted.pid);
        let sample = crate::infra::telemetry::get_service_specific_metrics("module-runtime");
        crate::infra::telemetry::update_service_resource("module-runtime", sample);

        info!(
            module_id = %module_id,
            pid = persisted.pid,
            "reattached running module process"
        );

        Ok(())
    }

    /// Persist runtime state to disk
    async fn save_state(&self) -> Result<(), ModuleRuntimeError> {
        let modules = self.running_modules.read().await;

        let states: Vec<PersistedModuleState> = modules
            .values()
            .map(|state| PersistedModuleState {
                module_id: state.module_id.to_string(),
                version: state.version.to_string(),
                pid: state.pid,
                port: state.port,
                started_at: state
                    .started_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                restart_count: state.restart_count,
                log_file: state.log_file.display().to_string(),
            })
            .collect();

        let state_file = self.state_dir.join(STATE_FILE_NAME);
        let contents = serde_json::to_string_pretty(&states).map_err(|e| {
            ModuleRuntimeError::InvalidState(format!("failed to serialize state: {}", e))
        })?;

        fs::write(&state_file, contents)
            .await
            .map_err(|e| ModuleRuntimeError::Io(format!("failed to write state file: {}", e)))?;

        Ok(())
    }

    /// Check if a process is still alive
    fn is_process_alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            match kill(Pid::from_raw(pid as i32), Signal::SIGCONT) {
                Ok(_) => true,
                Err(_) => false,
            }
        }

        #[cfg(not(unix))]
        {
            // On Windows, use different approach
            false
        }
    }

    /// Read port from module's config.toml if it exists
    async fn read_port_from_config(&self, module_path: &PathBuf) -> Option<u16> {
        let config_path = module_path.join("config.toml");

        match fs::read_to_string(&config_path).await {
            Ok(content) => match toml::from_str::<ModuleConfigPort>(&content) {
                Ok(config) => {
                    let port = config.server.and_then(|s| s.port);
                    if let Some(p) = port {
                        debug!(module_path = ?module_path, port = p, "read port from module config");
                    }
                    port
                }
                Err(e) => {
                    debug!(error = %e, "failed to parse module config.toml");
                    None
                }
            },
            Err(_) => {
                debug!(config_path = ?config_path, "no config.toml found in module");
                None
            }
        }
    }

    /// Kill a process
    async fn kill_process(&self, pid: u32) -> Result<(), ModuleRuntimeError> {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            // Try SIGTERM first
            if let Err(e) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                warn!("SIGTERM failed: {}, trying SIGKILL", e);
                kill(Pid::from_raw(pid as i32), Signal::SIGKILL).map_err(|e| {
                    ModuleRuntimeError::StopFailed {
                        module_id: "unknown".to_string(),
                        reason: format!("failed to kill process: {}", e),
                    }
                })?;
            }

            // Wait a bit for process to exit
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }

        #[cfg(not(unix))]
        {
            // On Windows, use taskkill
            let _ = Command::new("taskkill")
                .args(&["/PID", &pid.to_string(), "/F"])
                .output();
        }

        Ok(())
    }
}

#[async_trait]
impl ModuleRuntimePort for ProcessModuleRuntime {
    async fn start(
        &self,
        config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = config.module_id.to_string();

        // Check if already running
        {
            let modules = self.running_modules.read().await;
            if modules.contains_key(&module_id_str) {
                return Err(ModuleRuntimeError::AlreadyRunning {
                    module_id: module_id_str,
                });
            }
        }

        // Get installed module
        let installed = self
            .storage
            .load(&config.module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id_str.clone(),
            })?;

        let version = ModuleVersion(installed.manifest.version.clone());
        let module_path = PathBuf::from(&installed.path);

        // Read port from module's config.toml (if it exists)
        let port = self.read_port_from_config(&module_path).await;

        // Setup log file
        let log_dir = self.state_dir.join("logs");
        fs::create_dir_all(&log_dir).await.map_err(|e| {
            ModuleRuntimeError::Io(format!("failed to create log directory: {}", e))
        })?;

        let log_file = log_dir.join(format!("{}.log", module_id_str));
        let log_file_handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| ModuleRuntimeError::Io(format!("failed to open log file: {}", e)))?;

        // Build command
        let mut cmd = Command::new("cargo");
        cmd.arg("run")
            .current_dir(&module_path)
            .stdout(Stdio::from(log_file_handle.try_clone().unwrap()))
            .stderr(Stdio::from(log_file_handle))
            .stdin(Stdio::null());

        // Set environment variables
        for (key, value) in config.env_vars {
            cmd.env(key, value);
        }

        // Note: We don't set PORT env var - modules read from their own config

        // Spawn process
        let child = cmd.spawn().map_err(|e| ModuleRuntimeError::StartFailed {
            module_id: module_id_str.clone(),
            reason: format!("failed to spawn process: {}", e),
        })?;

        let pid = child.id();
        let started_at = SystemTime::now();

        info!(
            module_id = %config.module_id,
            pid = pid,
            port = ?port,
            "module process started"
        );

        // Update telemetry with real module process metrics
        use crate::infra::telemetry;
        telemetry::register_service_process("module-runtime", pid);
        let sample = telemetry::get_service_specific_metrics("module-runtime");
        telemetry::update_service_resource("module-runtime", sample);

        // Store running state
        let state = RunningModuleState {
            module_id: config.module_id.clone(),
            version: version.clone(),
            pid,
            port,
            started_at,
            restart_count: 0,
            log_file: log_file.clone(),
            child: Some(child),
        };

        {
            let mut modules = self.running_modules.write().await;
            modules.insert(module_id_str.clone(), state);
        }

        // Persist state
        if let Err(e) = self.save_state().await {
            error!("failed to persist runtime state: {}", e);
        }

        Ok(ModuleRuntimeInfo {
            module_id: config.module_id,
            version,
            status: ModuleRuntimeStatus::Running,
            pid: Some(pid),
            port,
            started_at: Some(started_at),
            stopped_at: None,
            restart_count: 0,
        })
    }

    async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        let module_id_str = module_id.to_string();

        let pid = {
            let mut modules = self.running_modules.write().await;
            let state =
                modules
                    .remove(&module_id_str)
                    .ok_or_else(|| ModuleRuntimeError::NotRunning {
                        module_id: module_id_str.clone(),
                    })?;

            state.pid
        };

        // Remove telemetry process tracking before killing to avoid stale samples
        crate::infra::telemetry::unregister_service_process("module-runtime", pid);

        // Kill the process
        self.kill_process(pid).await?;

        // Persist state
        if let Err(e) = self.save_state().await {
            error!("failed to persist runtime state: {}", e);
        }

        info!(module_id = %module_id, pid = pid, "module stopped");

        // Refresh telemetry sample after shutdown
        let sample = crate::infra::telemetry::get_service_specific_metrics("module-runtime");
        crate::infra::telemetry::update_service_resource("module-runtime", sample);

        Ok(())
    }

    async fn status(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let module_id_str = module_id.to_string();

        let state = modules
            .get(&module_id_str)
            .ok_or_else(|| ModuleRuntimeError::NotRunning {
                module_id: module_id_str.clone(),
            })?;

        // Check if process is still alive
        let status = if self.is_process_alive(state.pid) {
            ModuleRuntimeStatus::Running
        } else {
            ModuleRuntimeStatus::Failed
        };

        Ok(ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status,
            pid: Some(state.pid),
            port: state.port,
            started_at: Some(state.started_at),
            stopped_at: None,
            restart_count: state.restart_count,
        })
    }

    async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;

        let mut result = Vec::new();
        for state in modules.values() {
            let status = if self.is_process_alive(state.pid) {
                ModuleRuntimeStatus::Running
            } else {
                ModuleRuntimeStatus::Failed
            };

            result.push(ModuleRuntimeInfo {
                module_id: state.module_id.clone(),
                version: state.version.clone(),
                status,
                pid: Some(state.pid),
                port: state.port,
                started_at: Some(state.started_at),
                stopped_at: None,
                restart_count: state.restart_count,
            });
        }

        Ok(result)
    }

    async fn restart(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        // Get current config before stopping
        let (port, restart_count) = {
            let modules = self.running_modules.read().await;
            let module_id_str = module_id.to_string();

            let state =
                modules
                    .get(&module_id_str)
                    .ok_or_else(|| ModuleRuntimeError::NotRunning {
                        module_id: module_id_str.clone(),
                    })?;

            (state.port, state.restart_count + 1)
        };

        // Stop the module
        self.stop(module_id).await?;

        // Wait a bit
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Start again
        let config = ModuleStartConfig {
            module_id: module_id.clone(),
            port,
            env_vars: vec![],
            auto_restart: false,
        };

        let mut info = self.start(config).await?;
        info.restart_count = restart_count;

        Ok(info)
    }

    async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let module_id_str = module_id.to_string();

        let state = modules
            .get(&module_id_str)
            .ok_or_else(|| ModuleRuntimeError::NotRunning {
                module_id: module_id_str.clone(),
            })?;

        let contents = fs::read_to_string(&state.log_file)
            .await
            .map_err(|e| ModuleRuntimeError::Io(format!("failed to read log file: {}", e)))?;

        let lines: Vec<String> = contents.lines().map(String::from).collect();

        let result = if let Some(n) = tail {
            lines.iter().rev().take(n).rev().cloned().collect()
        } else {
            lines
        };

        Ok(result)
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedModuleState {
    module_id: String,
    version: String,
    pid: u32,
    port: Option<u16>,
    started_at: u64,
    restart_count: u32,
    log_file: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::module::{ChecksumAlgorithm, ModuleBundle, ModuleInstallResult};
    use crate::domain::module::{
        InstalledModule, ModuleArtifactDescriptor, ModuleChecksum, ModuleManifest,
        ModuleSignatureDescriptor, SignatureAlgorithm,
    };
    use crate::domain::module::{ModuleStorageError, ModuleStoragePort};
    use async_trait::async_trait;
    use semver::Version;
    use std::sync::Arc;
    use std::time::SystemTime;
    use uuid::Uuid;

    struct TestStorage {
        installed: InstalledModule,
    }

    #[async_trait]
    impl ModuleStoragePort for TestStorage {
        async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
            Ok(vec![self.installed.clone()])
        }

        async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError> {
            if &self.installed.manifest.id == id.as_str() {
                Ok(Some(self.installed.clone()))
            } else {
                Ok(None)
            }
        }

        async fn stage_and_activate(
            &self,
            _bundle: ModuleBundle,
        ) -> Result<ModuleInstallResult, ModuleStorageError> {
            Err(ModuleStorageError::InvalidState(
                "stage not implemented in test".to_string(),
            ))
        }

        async fn remove(&self, _id: &ModuleId) -> Result<(), ModuleStorageError> {
            Err(ModuleStorageError::InvalidState(
                "remove not implemented in test".to_string(),
            ))
        }
    }

    fn test_manifest() -> InstalledModule {
        InstalledModule {
            manifest: ModuleManifest {
                id: "fenrir-api".to_string(),
                version: Version::parse("1.2.3").unwrap(),
                title: None,
                description: None,
                fenrir_version: None,
                authors: vec![],
                license: None,
                artifact: ModuleArtifactDescriptor {
                    download_url: String::new(),
                    checksum: ModuleChecksum {
                        algorithm: ChecksumAlgorithm::Sha256,
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
            path: String::from("/dev/null"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn load_state_reattaches_running_process() {
        let tmp_dir = std::env::temp_dir().join(format!("fenrir-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let log_dir = tmp_dir.join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        let log_file = log_dir.join("fenrir-api.log");
        std::fs::write(&log_file, b"boot log").unwrap();

        let storage: Arc<dyn ModuleStoragePort> = Arc::new(TestStorage {
            installed: test_manifest(),
        });
        let runtime = ProcessModuleRuntime::new(Arc::clone(&storage), tmp_dir.clone());

        let state = PersistedModuleState {
            module_id: "fenrir-api".into(),
            version: "1.2.3".into(),
            pid: std::process::id(),
            port: Some(8080),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            restart_count: 2,
            log_file: log_file.display().to_string(),
        };

        let state_file = tmp_dir.join(STATE_FILE_NAME);
        tokio::fs::write(&state_file, serde_json::to_string(&vec![state]).unwrap())
            .await
            .unwrap();

        runtime.load_state().await.expect("state loads");

        let module_id = ModuleId::new("fenrir-api").unwrap();
        let info = runtime.status(&module_id).await.expect("status available");
        assert!(matches!(info.status, ModuleRuntimeStatus::Running));
        assert_eq!(info.pid, Some(std::process::id()));
        assert_eq!(info.port, Some(8080));
        assert_eq!(info.restart_count, 2);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
