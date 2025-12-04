use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimePort, ModuleRuntimeStatus,
    ModuleStartConfig, ModuleStoragePort, ModuleVersion,
};
use crate::utils::messages;

/// Classification result for executable validation.
enum ExecCheck {
    Compatible,
    Script,
    Foreign(&'static str),
    Unknown,
}

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
}

impl ProcessModuleRuntime {
    pub fn new(storage: Arc<dyn ModuleStoragePort>, state_dir: PathBuf) -> Self {
        if let Err(err) = std::fs::create_dir_all(&state_dir) {
            warn!(
                path = %state_dir.display(),
                error = %err,
                "{}",
                messages::infra::modules::runtime::process::STATE_DIR_PREPARE_FAILED
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
            debug!(
                "{}",
                messages::infra::modules::runtime::process::NO_STATE_FOUND
            );
            return Ok(());
        }

        match fs::read_to_string(&state_file).await {
            Ok(contents) => {
                match serde_json::from_str::<Vec<PersistedModuleState>>(&contents) {
                    Ok(states) => {
                        info!(
                            "{}",
                            messages::infra::modules::runtime::process::state_loaded(states.len())
                        );

                        // Check which modules are still running
                        for state in states {
                            if self.is_process_alive(state.pid) {
                                if let Err(err) = self.reattach_running_module(state).await {
                                    warn!(
                                        error = %err,
                                        "{}",
                                        messages::infra::modules::runtime::process::REATTACH_FAILED
                                    );
                                }
                            } else {
                                warn!(
                                    module_id = %state.module_id,
                                    pid = state.pid,
                                    "{}",
                                    messages::infra::modules::runtime::process::PROCESS_MISSING
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "{}",
                            messages::infra::modules::runtime::process::state_parse_failed(e)
                        );
                    }
                }
            }
            Err(e) => {
                error!(
                    "{}",
                    messages::infra::modules::runtime::process::state_read_failed(e)
                );
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
                    "{}",
                    messages::infra::modules::runtime::process::REATTACH_VERSION_MISMATCH
                );
            }
        } else {
            warn!(
                module_id = %module_id,
                "{}",
                messages::infra::modules::runtime::process::REATTACH_MISSING
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
            "{}",
            messages::infra::modules::runtime::process::REATTACH_RUNNING
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
            ModuleRuntimeError::InvalidState(
                messages::infra::modules::runtime::process::state_serialize_failed(e),
            )
        })?;

        fs::write(&state_file, contents).await.map_err(|e| {
            ModuleRuntimeError::Io(
                messages::infra::modules::runtime::process::state_write_failed(e),
            )
        })?;

        Ok(())
    }

    /// Check if a process is still alive
    fn is_process_alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_ok()
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
                        debug!(
                            module_path = ?module_path,
                            port = p,
                            "{}",
                            messages::infra::modules::runtime::process::MODULE_CONFIG_PORT
                        );
                    }
                    port
                }
                Err(e) => {
                    debug!(
                        error = %e,
                        "{}",
                        messages::infra::modules::runtime::process::MODULE_CONFIG_PARSE_FAILED
                    );
                    None
                }
            },
            Err(_) => {
                debug!(
                    config_path = ?config_path,
                    "{}",
                    messages::infra::modules::runtime::process::MODULE_CONFIG_MISSING
                );
                None
            }
        }
    }

    /// Check if a file is a valid executable binary for the current platform
    /// Accepts:
    /// - ELF on Linux
    /// - Mach-O / Universal binaries on macOS
    /// - Scripts with shebang (`#!`)
    fn classify_executable(path: &PathBuf) -> ExecCheck {
        let mut magic = [0u8; 4];
        let mut bytes_read = 0usize;

        if let Ok(mut file) = std::fs::File::open(path) {
            use std::io::Read;
            if let Ok(read) = file.read(&mut magic) {
                bytes_read = read;
            }
        }

        if bytes_read >= 4 {
            // 32/64-bit Mach-O and universal (fat) binaries
            let is_mach = (magic == [0xFE, 0xED, 0xFA, 0xCE])
                || (magic == [0xFE, 0xED, 0xFA, 0xCF])
                || (magic == [0xCE, 0xFA, 0xED, 0xFE])
                || (magic == [0xCF, 0xFA, 0xED, 0xFE])
                || (magic == [0xCA, 0xFE, 0xBA, 0xBE]);
            if is_mach {
                return if cfg!(target_os = "macos") {
                    ExecCheck::Compatible
                } else {
                    ExecCheck::Foreign("macos")
                };
            }
            // ELF
            if magic == [0x7F, b'E', b'L', b'F'] {
                return if cfg!(target_os = "linux") {
                    ExecCheck::Compatible
                } else {
                    ExecCheck::Foreign("linux")
                };
            }
        }

        // Detect shebang scripts without assuming text encoding
        if let Ok(mut file) = std::fs::File::open(path) {
            use std::io::Read;
            let mut shebang = [0u8; 2];
            if file.read(&mut shebang).map(|n| n == 2).unwrap_or(false) && shebang == [b'#', b'!'] {
                return ExecCheck::Script;
            }
        }

        ExecCheck::Unknown
    }

    /// Kill a process
    async fn kill_process(&self, pid: u32) -> Result<(), ModuleRuntimeError> {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            // Try SIGTERM first
            if let Err(e) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                warn!(
                    "{}",
                    messages::infra::modules::runtime::process::sigterm_failed(e)
                );
                kill(Pid::from_raw(pid as i32), Signal::SIGKILL).map_err(|e| {
                    ModuleRuntimeError::StopFailed {
                        module_id: "unknown".to_string(),
                        reason: messages::infra::modules::runtime::process::kill_failed(e),
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

        // Read port from module's config.toml (if it exists) unless provided
        let port = if config.port.is_some() {
            config.port
        } else {
            self.read_port_from_config(&module_path).await
        };

        // Setup log file
        let log_dir = self.state_dir.join("logs");
        fs::create_dir_all(&log_dir).await.map_err(|e| {
            ModuleRuntimeError::Io(
                messages::infra::modules::runtime::process::log_dir_create_failed(e),
            )
        })?;

        let log_file = log_dir.join(format!("{}.log", module_id_str));
        let log_file_handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| {
                ModuleRuntimeError::Io(
                    messages::infra::modules::runtime::process::log_file_open_failed(e),
                )
            })?;

        // Resolve executable path (binary inside module directory)
        // Try multiple common locations and naming patterns
        let binary_path = {
            let candidates = vec![
                // Direct: <module_dir>/<module_id>
                module_path.join(&module_id_str),
                // In bin subdirectory: <module_dir>/bin/<module_id>
                module_path.join("bin").join(&module_id_str),
                // With .exe extension (for cross-platform compatibility)
                module_path.join(format!("{}.exe", module_id_str)),
                module_path
                    .join("bin")
                    .join(format!("{}.exe", module_id_str)),
                // Alternative: just the module directory if it's a file
                module_path.clone(),
            ];

            let mut found_path: Option<PathBuf> = None;
            for candidate in candidates {
                if candidate.is_file() {
                    // Check if file is executable (Unix/macOS/Linux)
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(metadata) = std::fs::metadata(&candidate) {
                            let permissions = metadata.permissions();
                            // Check if file has execute permission
                            if permissions.mode() & 0o111 != 0 {
                                match Self::classify_executable(&candidate) {
                                    ExecCheck::Compatible | ExecCheck::Script => {
                                        found_path = Some(candidate);
                                        break;
                                    }
                                    ExecCheck::Foreign(target) => {
                                        return Err(ModuleRuntimeError::StartFailed {
                                            module_id: module_id_str.clone(),
                                            reason: messages::infra::modules::runtime::process::exec_foreign_target(
                                                target,
                                                std::env::consts::OS,
                                            ),
                                        });
                                    }
                                    ExecCheck::Unknown => {
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        // On Windows, just check if file exists
                        found_path = Some(candidate);
                        break;
                    }
                }
            }

            found_path.ok_or_else(|| ModuleRuntimeError::StartFailed {
                module_id: module_id_str.clone(),
                reason: messages::infra::modules::runtime::process::exec_not_found(
                    module_path.display(),
                    &module_id_str,
                ),
            })?
        };

        // Build command
        let mut cmd = Command::new(&binary_path);
        cmd.current_dir(&module_path)
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
            reason: messages::infra::modules::runtime::process::spawn_failed(e),
        })?;

        let pid = child.id();
        let started_at = SystemTime::now();

        info!(
            module_id = %config.module_id,
            pid = pid,
            port = ?port,
            "{}",
            messages::infra::modules::runtime::process::PROCESS_STARTED
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
        };

        {
            let mut modules = self.running_modules.write().await;
            modules.insert(module_id_str.clone(), state);
        }

        // Persist state
        if let Err(e) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(e)
            );
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
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(e)
            );
        }

        info!(
            module_id = %module_id,
            pid = pid,
            "{}",
            messages::infra::modules::runtime::process::MODULE_STOPPED
        );

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

        let contents = fs::read_to_string(&state.log_file).await.map_err(|e| {
            ModuleRuntimeError::Io(
                messages::infra::modules::runtime::process::log_file_read_failed(e),
            )
        })?;

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
    use crate::domain::module::{
        ChecksumAlgorithm, ModuleBundle, ModuleInstallResult, ModuleInstallSource,
    };
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
            _source: ModuleInstallSource,
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
            source: ModuleInstallSource::Distribution,
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
