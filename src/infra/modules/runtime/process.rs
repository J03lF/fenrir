use async_trait::async_trait;
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use http_body_util::BodyExt;
use reqwest::{Client as HttpClient, Method as ReqwestMethod};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command as TokioCommand;
use tokio::sync::{oneshot, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::domain::module::{
    ModuleId, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimeInstanceInfo, ModuleRuntimeKind,
    ModuleRuntimePort, ModuleRuntimeStatus, ModuleStartConfig, ModuleStoragePort, ModuleVersion,
};
use crate::services::ServiceDiagnostics;
use crate::utils::messages;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

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

const RUNTIME_MANIFEST_PATH: &str = ".fenrir/runtime.toml";
const PROCESS_TERMINATE_GRACE_MS: u64 = 2_000;
const PROCESS_FORCE_GRACE_MS: u64 = 1_000;
const PROCESS_EXIT_POLL_MS: u64 = 100;
const PRIMARY_INSTANCE_SUFFIX: &str = "primary";
const REPLICA_INSTANCE_PREFIX: &str = "replica";

#[derive(Debug, Deserialize)]
struct RuntimeManifest {
    #[serde(default)]
    runtime: RuntimeManifestSection,
    #[serde(default)]
    migrations: Option<RuntimeMigrationsSection>,
}

#[derive(Debug, Deserialize, Default)]
struct RuntimeManifestSection {
    #[serde(default)]
    mode: RuntimeMode,
    #[serde(default)]
    static_site: Option<RuntimeStaticSiteSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeMigrationsSection {
    /// Relative path to the migration directory within the module (default: "migrations").
    #[serde(default = "default_migrations_dir")]
    pub dir: String,
}

fn default_migrations_dir() -> String {
    "migrations".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum RuntimeMode {
    #[default]
    Auto,
    Process,
    StaticSite,
}

#[derive(Debug, Deserialize)]
struct RuntimeStaticSiteSection {
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default)]
    asset_roots: Vec<String>,
    #[serde(default)]
    index_file: Option<String>,
}

#[derive(Debug, Clone)]
struct StaticSiteProfile {
    asset_root: PathBuf,
    index_file: PathBuf,
    services_manifest: Option<PathBuf>,
}

impl StaticSiteProfile {
    fn new(asset_root: PathBuf, index_file: PathBuf) -> Self {
        let services_manifest = Self::discover_services_manifest(&asset_root);
        Self {
            asset_root,
            index_file,
            services_manifest,
        }
    }

    fn root(&self) -> &Path {
        &self.asset_root
    }

    fn discover_services_manifest(root: &Path) -> Option<PathBuf> {
        let candidate_dir = root.join(".fenrir");
        let candidates = [
            candidate_dir.join("services.json"),
            candidate_dir.join("services"),
            root.join("fenrir-services.json"),
            root.join("fenrir.services.json"),
        ];
        candidates.into_iter().find(|candidate| candidate.is_file())
    }
}

/// Process-based module runtime implementation
pub struct ProcessModuleRuntime {
    storage: Arc<dyn ModuleStoragePort>,
    state_dir: PathBuf,
    diagnostics: Arc<ServiceDiagnostics>,
    running_modules: Arc<RwLock<HashMap<String, RunningModuleState>>>,
    control_plane_base: String,
}

#[derive(Debug)]
struct RunningModuleState {
    instance_id: String,
    module_id: ModuleId,
    version: ModuleVersion,
    port: Option<u16>,
    started_at: SystemTime,
    restart_count: u32,
    log_file: PathBuf,
    env: Option<Vec<(String, String)>>,
    primary: bool,
    kind: RunningModuleKind,
}

#[derive(Debug)]
enum RunningModuleKind {
    Process {
        pid: u32,
    },
    StaticSite {
        shutdown: Option<oneshot::Sender<()>>,
        handle: JoinHandle<()>,
    },
}

impl ProcessModuleRuntime {
    fn primary_instance_id(module_id: &str) -> String {
        format!("{module_id}:{PRIMARY_INSTANCE_SUFFIX}")
    }

    fn replica_instance_id(module_id: &str, ordinal: usize) -> String {
        format!("{module_id}:{REPLICA_INSTANCE_PREFIX}:{ordinal}")
    }

    fn next_replica_instance_id(
        module_id: &str,
        modules: &HashMap<String, RunningModuleState>,
    ) -> String {
        let mut max_ordinal = 0usize;
        for state in modules
            .values()
            .filter(|state| state.module_id.as_str() == module_id)
        {
            if let Some(ordinal) = Self::replica_ordinal(&state.instance_id) {
                max_ordinal = max_ordinal.max(ordinal);
            }
        }
        Self::replica_instance_id(module_id, max_ordinal.saturating_add(1))
    }

    fn replica_ordinal(instance_id: &str) -> Option<usize> {
        let (_, ordinal) = instance_id.rsplit_once(&format!(":{REPLICA_INSTANCE_PREFIX}:"))?;
        ordinal.parse::<usize>().ok()
    }

    fn build_log_file_path(&self, module_id: &str, primary: bool, instance_id: &str) -> PathBuf {
        let log_dir = self.state_dir.join("logs");
        if primary {
            log_dir.join(format!("{module_id}.log"))
        } else {
            let sanitized = instance_id.replace(':', "__");
            log_dir.join(format!("{module_id}__{sanitized}.log"))
        }
    }

    fn select_primary_instance<'a>(
        module_id: &str,
        modules: &'a HashMap<String, RunningModuleState>,
    ) -> Option<&'a RunningModuleState> {
        modules
            .values()
            .filter(|state| state.module_id.as_str() == module_id)
            .max_by(|left, right| {
                left.primary.cmp(&right.primary).then_with(|| {
                    left.started_at
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .cmp(
                            &right
                                .started_at
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default(),
                        )
                })
            })
    }

    fn module_instances<'a>(
        module_id: &str,
        modules: &'a HashMap<String, RunningModuleState>,
    ) -> Vec<&'a RunningModuleState> {
        let mut states = modules
            .values()
            .filter(|state| state.module_id.as_str() == module_id)
            .collect::<Vec<_>>();
        states.sort_by(|left, right| {
            right
                .primary
                .cmp(&left.primary)
                .then_with(|| left.instance_id.cmp(&right.instance_id))
        });
        states
    }

    fn info_from_state(&self, state: &RunningModuleState) -> ModuleRuntimeInfo {
        let (status, pid, kind) = match &state.kind {
            RunningModuleKind::Process { pid } => {
                let status = if self.is_process_alive(*pid) {
                    ModuleRuntimeStatus::Running
                } else {
                    ModuleRuntimeStatus::Failed
                };
                (status, Some(*pid), ModuleRuntimeKind::Process)
            }
            RunningModuleKind::StaticSite { handle, .. } => {
                let status = if handle.is_finished() {
                    ModuleRuntimeStatus::Failed
                } else {
                    ModuleRuntimeStatus::Running
                };
                (status, None, ModuleRuntimeKind::StaticSite)
            }
        };

        ModuleRuntimeInfo {
            module_id: state.module_id.clone(),
            version: state.version.clone(),
            status,
            kind,
            pid,
            port: state.port,
            started_at: Some(state.started_at),
            stopped_at: None,
            restart_count: state.restart_count,
        }
    }

    pub fn new(
        storage: Arc<dyn ModuleStoragePort>,
        state_dir: PathBuf,
        diagnostics: Arc<ServiceDiagnostics>,
        control_plane_base: String,
    ) -> Self {
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
            diagnostics,
            running_modules: Arc::new(RwLock::new(HashMap::new())),
            control_plane_base,
        }
    }

    fn reset_log_file(path: &Path) -> Result<(), ModuleRuntimeError> {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map(|_| ())
            .map_err(|err| {
                ModuleRuntimeError::Io(
                    messages::infra::modules::runtime::process::log_file_reset_failed(err),
                )
            })
    }

    fn log_timestamp() -> String {
        OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown-timestamp".to_string())
    }

    async fn write_static_log_entry(module: &ModuleId, log_file: &Path, port: u16, root: &Path) {
        let line = format!(
            "{} [static] module {} serving assets from {} on http://127.0.0.1:{port}",
            Self::log_timestamp(),
            module,
            root.display()
        );
        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .await
        {
            Ok(mut file) => {
                if let Err(err) = file.write_all(line.as_bytes()).await {
                    warn!(
                        module = %module,
                        path = %log_file.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::runtime::process::log_file_write_failed(&err)
                    );
                    return;
                }
                if let Err(err) = file.write_all(b"\n").await {
                    warn!(
                        module = %module,
                        path = %log_file.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::runtime::process::log_file_write_failed(&err)
                    );
                }
            }
            Err(err) => {
                warn!(
                    module = %module,
                    path = %log_file.display(),
                    error = %err,
                    "{}",
                    messages::infra::modules::runtime::process::log_file_write_failed(&err)
                );
            }
        }
    }

    fn resolve_bootstrap_binary(module_id: &ModuleId) -> Result<PathBuf, ModuleRuntimeError> {
        let mut exe = std::env::current_exe().map_err(|err| ModuleRuntimeError::StartFailed {
            module_id: module_id.to_string(),
            reason: messages::infra::modules::runtime::process::bootstrap_exe_resolve_failed(err),
        })?;
        exe.set_file_name("fenrir-module-kit");
        #[cfg(windows)]
        {
            exe.set_extension("exe");
        }
        if exe.exists() {
            return Ok(exe);
        }
        Err(ModuleRuntimeError::StartFailed {
            module_id: module_id.to_string(),
            reason: messages::infra::modules::runtime::process::BOOTSTRAP_BINARY_MISSING
                .to_string(),
        })
    }

    async fn run_bootstrap_script(
        &self,
        module_id: &ModuleId,
        module_path: &Path,
        env_vars: &[(String, String)],
    ) -> Result<(), ModuleRuntimeError> {
        let binary = Self::resolve_bootstrap_binary(module_id)?;
        let mut cmd = TokioCommand::new(&binary);
        cmd.arg("init")
            .arg("--module-root")
            .arg(module_path)
            .current_dir(module_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear();
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        let status = cmd
            .status()
            .await
            .map_err(|err| ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: messages::infra::modules::runtime::process::bootstrap_spawn_failed(err),
            })?;
        if !status.success() {
            return Err(ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: messages::infra::modules::runtime::process::bootstrap_failed(status),
            });
        }
        Ok(())
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
        let instance_id = if persisted.instance_id.trim().is_empty() {
            Self::primary_instance_id(module_id.as_str())
        } else {
            persisted.instance_id.clone()
        };
        let state = RunningModuleState {
            instance_id: instance_id.clone(),
            module_id: module_id.clone(),
            version: expected_version.clone(),
            port: persisted.port,
            started_at,
            restart_count: persisted.restart_count,
            log_file: PathBuf::from(&persisted.log_file),
            env: None,
            primary: persisted.primary,
            kind: RunningModuleKind::Process { pid: persisted.pid },
        };

        {
            let mut modules = self.running_modules.write().await;
            modules.insert(instance_id, state);
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
            .filter_map(PersistedModuleState::from_running_state)
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

    /// Check if a process is still alive (not exited and not a zombie)
    fn is_process_alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            use nix::sys::signal::kill;
            use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
            use nix::unistd::Pid;

            let target = Pid::from_raw(pid as i32);

            // First try to reap the process if it's a zombie (non-blocking waitpid).
            // This only works if we're the parent process of the target.
            match waitpid(target, Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(_, _)) | Ok(WaitStatus::Signaled(_, _, _)) => {
                    // Process has exited and we just reaped it
                    return false;
                }
                Ok(WaitStatus::StillAlive) => {
                    // Process is still running (we're the parent)
                    return true;
                }
                Err(nix::errno::Errno::ECHILD) => {
                    // Not our child process, fall through to kill(0) check
                }
                _ => {}
            }

            // Use signal 0 (None) to check if process exists without sending a real signal.
            // Note: This will return true for zombies we didn't spawn, but that's rare.
            kill(target, None).is_ok()
        }

        #[cfg(not(unix))]
        {
            // On Windows, use different approach
            false
        }
    }

    fn sanitized_host_env() -> Vec<(String, String)> {
        env::vars()
            .filter(|(key, _)| !key.starts_with("FENRIR_"))
            .collect()
    }

    fn load_runtime_manifest(module_path: &Path) -> Option<RuntimeManifest> {
        let manifest_path = module_path.join(RUNTIME_MANIFEST_PATH);
        if !manifest_path.exists() {
            return None;
        }
        match std::fs::read_to_string(&manifest_path) {
            Ok(contents) => match toml::from_str::<RuntimeManifest>(&contents) {
                Ok(manifest) => Some(manifest),
                Err(err) => {
                    warn!(
                        path = %manifest_path.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::runtime::process::runtime_manifest_parse_failed(&err)
                    );
                    None
                }
            },
            Err(err) => {
                warn!(
                    path = %manifest_path.display(),
                    error = %err,
                    "{}",
                    messages::infra::modules::runtime::process::runtime_manifest_read_failed(&err)
                );
                None
            }
        }
    }

    fn static_profile_from_manifest(
        module_id: &ModuleId,
        module_path: &Path,
        manifest: &RuntimeManifest,
    ) -> Result<Option<StaticSiteProfile>, ModuleRuntimeError> {
        let Some(static_cfg) = manifest.runtime.static_site.as_ref() else {
            return Ok(None);
        };
        if let Some(entry) = static_cfg
            .entrypoint
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            let resolved = module_path.join(entry);
            if resolved.is_file() {
                let root = resolved
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| module_path.to_path_buf());
                return Ok(Some(StaticSiteProfile::new(root, resolved)));
            } else {
                return Err(ModuleRuntimeError::StartFailed {
                    module_id: module_id.to_string(),
                    reason: messages::infra::modules::runtime::process::static_entrypoint_missing(
                        resolved.display(),
                    ),
                });
            }
        }
        let index_name = static_cfg
            .index_file
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("index.html");
        if static_cfg.asset_roots.is_empty() {
            return Err(ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: messages::infra::modules::runtime::process::static_asset_roots_missing(),
            });
        }
        for root in &static_cfg.asset_roots {
            let trimmed = root.trim();
            if trimmed.is_empty() {
                continue;
            }
            let candidate = module_path.join(trimmed);
            if let Some(profile) = Self::build_profile_from_root(&candidate, index_name) {
                return Ok(Some(profile));
            }
        }
        Err(ModuleRuntimeError::StartFailed {
            module_id: module_id.to_string(),
            reason: messages::infra::modules::runtime::process::static_assets_not_found(
                module_path.display(),
            ),
        })
    }

    fn discover_static_profile(module_path: &Path) -> Option<StaticSiteProfile> {
        let mut candidates = Vec::new();
        let primary_dirs = [
            "dist",
            "dist/browser",
            "dist/public",
            "build",
            "build/browser",
            "public",
            "static",
            "www",
            "out",
            "browser",
        ];
        for dir in primary_dirs {
            candidates.push(module_path.join(dir));
        }

        if let Ok(entries) = std::fs::read_dir(module_path) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_lowercase();
                        if name.starts_with("dist")
                            || name.ends_with("dist")
                            || name.contains("browser")
                        {
                            candidates.push(entry.path());
                        }
                        candidates.push(entry.path().join("dist"));
                        candidates.push(entry.path().join("dist/browser"));
                    }
                }
            }
        }

        for group in ["apps", "packages", "projects", "modules"] {
            let container = module_path.join(group);
            if let Ok(entries) = std::fs::read_dir(&container) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_dir() {
                            let path = entry.path();
                            candidates.push(path.join("dist"));
                            candidates.push(path.join("dist/browser"));
                            candidates.push(path.join("build"));
                            candidates.push(path.join("public"));
                        }
                    }
                }
            }
        }

        for candidate in candidates {
            if let Some(profile) = Self::build_profile_from_root(&candidate, "index.html") {
                return Some(profile);
            }
        }
        None
    }

    fn build_profile_from_root(root: &Path, index_name: &str) -> Option<StaticSiteProfile> {
        if !root.exists() || !root.is_dir() {
            return None;
        }
        let direct = root.join(index_name);
        if direct.is_file() {
            return Some(StaticSiteProfile::new(root.to_path_buf(), direct));
        }
        let browser = root.join("browser");
        if browser.is_dir() {
            let browser_index = browser.join(index_name);
            if browser_index.is_file() {
                return Some(StaticSiteProfile::new(browser, browser_index));
            }
        }
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let nested_root = entry.path();
                        let nested_index = nested_root.join(index_name);
                        if nested_index.is_file() {
                            return Some(StaticSiteProfile::new(nested_root, nested_index));
                        }
                    }
                }
            }
        }
        None
    }

    fn locate_process_binary(
        module_path: &Path,
        module_id: &str,
    ) -> Result<Option<PathBuf>, ModuleRuntimeError> {
        let candidates = vec![
            module_path.join(module_id),
            module_path.join("bin").join(module_id),
            module_path.join(format!("{module_id}.exe")),
            module_path.join("bin").join(format!("{module_id}.exe")),
            module_path.to_path_buf(),
        ];

        for candidate in candidates {
            if candidate.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = std::fs::metadata(&candidate) {
                        let permissions = metadata.permissions();
                        if permissions.mode() & 0o111 != 0 {
                            match Self::classify_executable(&candidate) {
                                ExecCheck::Compatible | ExecCheck::Script => {
                                    return Ok(Some(candidate));
                                }
                                ExecCheck::Foreign(target) => {
                                    return Err(ModuleRuntimeError::StartFailed {
                                        module_id: module_id.to_string(),
                                        reason: messages::infra::modules::runtime::process::exec_foreign_target(
                                            target,
                                            std::env::consts::OS,
                                        ),
                                    });
                                }
                                ExecCheck::Unknown => continue,
                            }
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    return Ok(Some(candidate));
                }
            }
        }
        Ok(None)
    }

    fn build_static_router(
        &self,
        module_id: &ModuleId,
        profile: StaticSiteProfile,
        diagnostics: Arc<ServiceDiagnostics>,
        log_file: PathBuf,
    ) -> Router {
        let state = StaticRouterState::new(
            module_id,
            profile,
            diagnostics,
            self.control_plane_base.clone(),
            log_file,
        );
        Router::new()
            .route("/live", get(static_health_handler))
            .route("/ready", get(static_health_handler))
            .route("/healthz", get(static_health_handler))
            .route("/.fenrir/services", get(static_services_handler))
            .route("/gateway/*rest", any(static_gateway_proxy_handler))
            .fallback(get(static_asset_handler))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                static_request_logger,
            ))
            .with_state(state)
    }

    async fn try_start_static_site(
        &self,
        module_id: &ModuleId,
        instance_id: String,
        primary: bool,
        port: Option<u16>,
        version: ModuleVersion,
        log_file: PathBuf,
        env: Vec<(String, String)>,
        profile: StaticSiteProfile,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let bind_port = port.unwrap_or(0);
        let listener = TcpListener::bind(("127.0.0.1", bind_port))
            .await
            .map_err(|err| ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: format!("failed to bind static module port: {err}"),
            })?;
        let actual_addr = listener
            .local_addr()
            .map_err(|err| ModuleRuntimeError::StartFailed {
                module_id: module_id.to_string(),
                reason: format!("failed to determine static module address: {err}"),
            })?;

        let router = self.build_static_router(
            module_id,
            profile.clone(),
            Arc::clone(&self.diagnostics),
            log_file.clone(),
        );
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let module_for_log = module_id.to_string();
        let handle = tokio::spawn(async move {
            let server =
                axum::serve(listener, router.into_make_service()).with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                });
            if let Err(err) = server.await {
                error!(
                    module = %module_for_log,
                    error = %err,
                    "static module server exited with error"
                );
            }
        });

        let started_at = SystemTime::now();
        let state = RunningModuleState {
            instance_id: instance_id.clone(),
            module_id: module_id.clone(),
            version: version.clone(),
            port: Some(actual_addr.port()),
            started_at,
            restart_count: 0,
            log_file: log_file.clone(),
            env: Some(env),
            primary,
            kind: RunningModuleKind::StaticSite {
                shutdown: Some(shutdown_tx),
                handle,
            },
        };

        {
            let mut modules = self.running_modules.write().await;
            modules.insert(instance_id, state);
        }

        if let Err(e) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(e)
            );
        }

        info!(
            module_id = %module_id,
            port = actual_addr.port(),
            root = %profile.root().display(),
            "{}",
            messages::infra::modules::runtime::process::STATIC_SERVER_STARTED
        );
        Self::write_static_log_entry(module_id, &log_file, actual_addr.port(), profile.root())
            .await;

        Ok(ModuleRuntimeInfo {
            module_id: module_id.clone(),
            version,
            status: ModuleRuntimeStatus::Running,
            kind: ModuleRuntimeKind::StaticSite,
            pid: None,
            port: Some(actual_addr.port()),
            started_at: Some(started_at),
            stopped_at: None,
            restart_count: 0,
        })
    }

    async fn start_specific_instance(
        &self,
        module_id: &ModuleId,
        instance_id: String,
        primary: bool,
        requested_port: Option<u16>,
        env_vars: Vec<(String, String)>,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let installed = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id_str.clone(),
            })?;

        let version = ModuleVersion(installed.manifest.version.clone());
        let module_path = PathBuf::from(&installed.path);
        let port = if requested_port.is_some() {
            requested_port
        } else {
            self.read_port_from_config(&module_path).await
        };

        let mut full_env = Self::sanitized_host_env();
        full_env.extend(env_vars);

        let log_dir = self.state_dir.join("logs");
        fs::create_dir_all(&log_dir).await.map_err(|e| {
            ModuleRuntimeError::Io(
                messages::infra::modules::runtime::process::log_dir_create_failed(e),
            )
        })?;

        let log_file = self.build_log_file_path(&module_id_str, primary, &instance_id);
        Self::reset_log_file(&log_file)?;

        let runtime_manifest = Self::load_runtime_manifest(&module_path);
        let runtime_mode = runtime_manifest
            .as_ref()
            .map(|manifest| manifest.runtime.mode)
            .unwrap_or(RuntimeMode::Auto);
        let manifest_static_profile =
            if matches!(runtime_mode, RuntimeMode::Auto | RuntimeMode::StaticSite) {
                if let Some(manifest) = runtime_manifest.as_ref() {
                    Self::static_profile_from_manifest(module_id, &module_path, manifest)?
                } else {
                    None
                }
            } else {
                None
            };
        let mut static_profile = manifest_static_profile;
        if static_profile.is_none()
            && matches!(runtime_mode, RuntimeMode::Auto | RuntimeMode::StaticSite)
        {
            static_profile = Self::discover_static_profile(&module_path);
        }

        if matches!(runtime_mode, RuntimeMode::StaticSite) {
            let profile = static_profile
                .take()
                .ok_or_else(|| ModuleRuntimeError::StartFailed {
                    module_id: module_id_str.clone(),
                    reason: messages::infra::modules::runtime::process::static_assets_not_found(
                        module_path.display(),
                    ),
                })?;
            return self
                .try_start_static_site(
                    module_id,
                    instance_id,
                    primary,
                    requested_port,
                    version.clone(),
                    log_file.clone(),
                    full_env.clone(),
                    profile,
                )
                .await;
        }

        let binary_candidate = Self::locate_process_binary(&module_path, &module_id_str)?;
        if binary_candidate.is_none() && matches!(runtime_mode, RuntimeMode::Auto) {
            if let Some(profile) = static_profile {
                return self
                    .try_start_static_site(
                        module_id,
                        instance_id,
                        primary,
                        requested_port,
                        version.clone(),
                        log_file.clone(),
                        full_env.clone(),
                        profile,
                    )
                    .await;
            }
        }

        let binary_path = binary_candidate.ok_or_else(|| ModuleRuntimeError::StartFailed {
            module_id: module_id_str.clone(),
            reason: messages::infra::modules::runtime::process::exec_not_found(
                module_path.display(),
                &module_id_str,
            ),
        })?;

        let log_file_handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(|e| {
                ModuleRuntimeError::Io(
                    messages::infra::modules::runtime::process::log_file_open_failed(e),
                )
            })?;

        self.run_bootstrap_script(module_id, &module_path, &full_env)
            .await?;

        let mut cmd = Command::new(&binary_path);
        cmd.current_dir(&module_path)
            .stdout(Stdio::from(log_file_handle.try_clone().unwrap()))
            .stderr(Stdio::from(log_file_handle))
            .stdin(Stdio::null())
            .env_clear();
        for (key, value) in &full_env {
            cmd.env(key, value);
        }

        let child = cmd.spawn().map_err(|e| ModuleRuntimeError::StartFailed {
            module_id: module_id_str.clone(),
            reason: messages::infra::modules::runtime::process::spawn_failed(e),
        })?;

        let pid = child.id();
        let started_at = SystemTime::now();

        info!(
            module_id = %module_id,
            instance_id = %instance_id,
            pid = pid,
            port = ?port,
            "{}",
            messages::infra::modules::runtime::process::PROCESS_STARTED
        );

        use crate::infra::telemetry;
        telemetry::register_service_process("module-runtime", pid);
        let sample = telemetry::get_service_specific_metrics("module-runtime");
        telemetry::update_service_resource("module-runtime", sample);

        let state = RunningModuleState {
            instance_id: instance_id.clone(),
            module_id: module_id.clone(),
            version: version.clone(),
            port,
            started_at,
            restart_count: 0,
            log_file: log_file.clone(),
            env: Some(full_env.clone()),
            primary,
            kind: RunningModuleKind::Process { pid },
        };

        {
            let mut modules = self.running_modules.write().await;
            modules.insert(instance_id, state);
        }

        if let Err(e) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(e)
            );
        }

        Ok(ModuleRuntimeInfo {
            module_id: module_id.clone(),
            version,
            status: ModuleRuntimeStatus::Running,
            kind: ModuleRuntimeKind::Process,
            pid: Some(pid),
            port,
            started_at: Some(started_at),
            stopped_at: None,
            restart_count: 0,
        })
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

            let target = Pid::from_raw(pid as i32);
            let term_result = kill(target, Signal::SIGTERM);
            if let Err(err) = term_result {
                warn!(
                    "{}",
                    messages::infra::modules::runtime::process::sigterm_failed(err)
                );
            } else if self
                .wait_for_exit(pid, Duration::from_millis(PROCESS_TERMINATE_GRACE_MS))
                .await
            {
                return Ok(());
            }

            if self.is_process_alive(pid) {
                warn!(
                    pid = pid,
                    "{}",
                    messages::infra::modules::runtime::process::SIGTERM_TIMEOUT
                );
                kill(target, Signal::SIGKILL).map_err(|err| ModuleRuntimeError::StopFailed {
                    module_id: "unknown".to_string(),
                    reason: messages::infra::modules::runtime::process::kill_failed(err),
                })?;
                if self
                    .wait_for_exit(pid, Duration::from_millis(PROCESS_FORCE_GRACE_MS))
                    .await
                {
                    return Ok(());
                }
                return Err(ModuleRuntimeError::StopFailed {
                    module_id: "unknown".to_string(),
                    reason: messages::infra::modules::runtime::process::PROCESS_EXIT_TIMEOUT
                        .to_string(),
                });
            }
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

impl ProcessModuleRuntime {
    async fn wait_for_exit(&self, pid: u32, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.is_process_alive(pid) {
            if start.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(PROCESS_EXIT_POLL_MS)).await;
        }
        true
    }

    async fn stop_instance_by_id(
        &self,
        module_id: &ModuleId,
        instance_id: &str,
    ) -> Result<(), ModuleRuntimeError> {
        let state = {
            let mut modules = self.running_modules.write().await;
            modules
                .remove(instance_id)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id.to_string(),
                })?
        };

        match state.kind {
            RunningModuleKind::Process { pid } => {
                crate::infra::telemetry::unregister_service_process("module-runtime", pid);
                self.kill_process(pid).await?;
                let sample =
                    crate::infra::telemetry::get_service_specific_metrics("module-runtime");
                crate::infra::telemetry::update_service_resource("module-runtime", sample);
            }
            RunningModuleKind::StaticSite {
                mut shutdown,
                handle,
            } => {
                if let Some(tx) = shutdown.take() {
                    let _ = tx.send(());
                }
                if let Err(err) = handle.await {
                    warn!(
                        module_id = %module_id,
                        instance_id = %state.instance_id,
                        error = %err,
                        "failed to await static module task"
                    );
                }
            }
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
        {
            let modules = self.running_modules.read().await;
            if Self::select_primary_instance(&module_id_str, &modules).is_some() {
                return Err(ModuleRuntimeError::AlreadyRunning {
                    module_id: module_id_str,
                });
            }
        }

        self.start_specific_instance(
            &config.module_id,
            Self::primary_instance_id(config.module_id.as_str()),
            true,
            config.port,
            config.env_vars,
        )
        .await
    }

    async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let instance_ids = {
            let modules = self.running_modules.read().await;
            let ids = Self::module_instances(&module_id_str, &modules)
                .into_iter()
                .map(|state| state.instance_id.clone())
                .collect::<Vec<_>>();
            if ids.is_empty() {
                return Err(ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                });
            }
            ids
        };

        for instance_id in instance_ids {
            let state = {
                let mut modules = self.running_modules.write().await;
                modules
                    .remove(&instance_id)
                    .ok_or_else(|| ModuleRuntimeError::NotRunning {
                        module_id: module_id_str.clone(),
                    })?
            };

            match state.kind {
                RunningModuleKind::Process { pid } => {
                    crate::infra::telemetry::unregister_service_process("module-runtime", pid);
                    self.kill_process(pid).await?;
                    info!(
                        module_id = %module_id,
                        instance_id = %state.instance_id,
                        pid = pid,
                        "{}",
                        messages::infra::modules::runtime::process::MODULE_STOPPED
                    );
                    let sample =
                        crate::infra::telemetry::get_service_specific_metrics("module-runtime");
                    crate::infra::telemetry::update_service_resource("module-runtime", sample);
                }
                RunningModuleKind::StaticSite {
                    mut shutdown,
                    handle,
                    ..
                } => {
                    if let Some(tx) = shutdown.take() {
                        let _ = tx.send(());
                    }
                    if let Err(err) = handle.await {
                        warn!(
                            module_id = %module_id,
                            instance_id = %state.instance_id,
                            error = %err,
                            "failed to await static module task"
                        );
                    }
                    info!(
                        module_id = %module_id,
                        instance_id = %state.instance_id,
                        "{}",
                        messages::infra::modules::runtime::process::STATIC_SERVER_STOPPED
                    );
                }
            }
        }

        if let Err(e) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(e)
            );
        }

        Ok(())
    }

    async fn status(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let module_id_str = module_id.to_string();
        let state = Self::select_primary_instance(&module_id_str, &modules).ok_or_else(|| {
            ModuleRuntimeError::NotRunning {
                module_id: module_id_str.clone(),
            }
        })?;
        Ok(self.info_from_state(state))
    }

    async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let mut seen = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for state in modules.values() {
            let module_key = state.module_id.to_string();
            if !seen.insert(module_key.clone()) {
                continue;
            }
            if let Some(primary) = Self::select_primary_instance(&module_key, &modules) {
                result.push(self.info_from_state(primary));
            }
        }
        Ok(result)
    }

    async fn restart(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let instances = self.list_instances(module_id).await?;
        let mut primary_info = None;
        for instance in instances {
            let restarted = self
                .restart_instance(module_id, &instance.instance_id)
                .await?;
            if primary_info.is_none() {
                primary_info = Some(restarted);
            }
        }
        primary_info.ok_or_else(|| ModuleRuntimeError::NotRunning {
            module_id: module_id.to_string(),
        })
    }

    async fn list_instances(
        &self,
        module_id: &ModuleId,
    ) -> Result<Vec<ModuleRuntimeInstanceInfo>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let states = Self::module_instances(module_id.as_str(), &modules);
        if states.is_empty() {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id.to_string(),
            });
        }
        Ok(states
            .into_iter()
            .map(|state| ModuleRuntimeInstanceInfo {
                instance_id: state.instance_id.clone(),
                primary: state.primary,
                runtime: self.info_from_state(state),
            })
            .collect())
    }

    async fn restart_instance(
        &self,
        module_id: &ModuleId,
        instance_id: &str,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        let module_id_str = module_id.to_string();
        let state = {
            let mut modules = self.running_modules.write().await;
            modules
                .remove(instance_id)
                .ok_or_else(|| ModuleRuntimeError::NotRunning {
                    module_id: module_id_str.clone(),
                })?
        };
        if state.module_id != *module_id {
            return Err(ModuleRuntimeError::NotRunning {
                module_id: module_id_str,
            });
        }

        let port = state.port;
        let env_vars = state.env.clone().unwrap_or_default();
        let primary = state.primary;
        let restart_count = state.restart_count.saturating_add(1);
        let instance_id_owned = state.instance_id.clone();

        match state.kind {
            RunningModuleKind::Process { pid } => {
                crate::infra::telemetry::unregister_service_process("module-runtime", pid);
                self.kill_process(pid).await?;
            }
            RunningModuleKind::StaticSite {
                mut shutdown,
                handle,
            } => {
                if let Some(tx) = shutdown.take() {
                    let _ = tx.send(());
                }
                if let Err(err) = handle.await {
                    warn!(
                        module_id = %module_id,
                        instance_id = %instance_id_owned,
                        error = %err,
                        "failed to await static module task during restart"
                    );
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let mut info = self
            .start_specific_instance(
                module_id,
                instance_id_owned.clone(),
                primary,
                port,
                env_vars,
            )
            .await?;
        info.restart_count = restart_count;
        {
            let mut modules = self.running_modules.write().await;
            if let Some(restarted) = modules.get_mut(&instance_id_owned) {
                restarted.restart_count = restart_count;
            }
        }
        if let Err(err) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(err)
            );
        }
        Ok(info)
    }

    async fn reconcile_instances(
        &self,
        config: ModuleStartConfig,
        desired_instances: usize,
    ) -> Result<Vec<ModuleRuntimeInstanceInfo>, ModuleRuntimeError> {
        if desired_instances == 0 {
            let _ = self.stop(&config.module_id).await;
            return Ok(Vec::new());
        }

        let module_id_str = config.module_id.to_string();
        let current_instances = self
            .list_instances(&config.module_id)
            .await
            .unwrap_or_default();
        let current_count = current_instances.len();

        if current_count == 0 {
            let _ = self.start(config.clone()).await?;
        } else if current_count < desired_instances {
            let additional = desired_instances - current_count;
            for _ in 0..additional {
                let instance_id = {
                    let modules = self.running_modules.read().await;
                    Self::next_replica_instance_id(&module_id_str, &modules)
                };
                self.start_specific_instance(
                    &config.module_id,
                    instance_id,
                    false,
                    None,
                    config.env_vars.clone(),
                )
                .await?;
            }
        } else if current_count > desired_instances {
            let remove_count = current_count - desired_instances;
            let instances = self.list_instances(&config.module_id).await?;
            for instance in instances
                .into_iter()
                .filter(|entry| !entry.primary)
                .rev()
                .take(remove_count)
            {
                self.stop_instance_by_id(&config.module_id, &instance.instance_id)
                    .await?;
            }
        }

        if let Err(err) = self.save_state().await {
            error!(
                "{}",
                messages::infra::modules::runtime::process::state_persist_failed(err)
            );
        }
        self.list_instances(&config.module_id).await
    }

    async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let module_id_str = module_id.to_string();
        let state = Self::select_primary_instance(&module_id_str, &modules).ok_or_else(|| {
            ModuleRuntimeError::NotRunning {
                module_id: module_id_str.clone(),
            }
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

    async fn env(&self, module_id: &ModuleId) -> Result<Vec<(String, String)>, ModuleRuntimeError> {
        let modules = self.running_modules.read().await;
        let module_id_str = module_id.to_string();
        let state = Self::select_primary_instance(&module_id_str, &modules).ok_or_else(|| {
            ModuleRuntimeError::NotRunning {
                module_id: module_id_str.clone(),
            }
        })?;
        match &state.env {
            Some(env) => Ok(env.clone()),
            None => Err(ModuleRuntimeError::EnvUnavailable {
                module_id: module_id_str,
            }),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PersistedModuleState {
    #[serde(default)]
    instance_id: String,
    #[serde(default = "default_true")]
    primary: bool,
    module_id: String,
    version: String,
    pid: u32,
    port: Option<u16>,
    started_at: u64,
    restart_count: u32,
    log_file: String,
}

impl PersistedModuleState {
    fn from_running_state(state: &RunningModuleState) -> Option<Self> {
        match &state.kind {
            RunningModuleKind::Process { pid } => Some(Self {
                instance_id: state.instance_id.clone(),
                primary: state.primary,
                module_id: state.module_id.to_string(),
                version: state.version.to_string(),
                pid: *pid,
                port: state.port,
                started_at: state
                    .started_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                restart_count: state.restart_count,
                log_file: state.log_file.display().to_string(),
            }),
            RunningModuleKind::StaticSite { .. } => None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone)]
struct StaticRouterState {
    module: Arc<String>,
    root: Arc<PathBuf>,
    index: Arc<PathBuf>,
    services_manifest: Option<Arc<PathBuf>>,
    diagnostics: Arc<ServiceDiagnostics>,
    metric_id: Arc<String>,
    control_plane_base: Arc<String>,
    http_client: HttpClient,
    log_file: Arc<PathBuf>,
}

impl StaticRouterState {
    fn new(
        module_id: &ModuleId,
        profile: StaticSiteProfile,
        diagnostics: Arc<ServiceDiagnostics>,
        control_plane_base: String,
        log_file: PathBuf,
    ) -> Self {
        let module = Arc::new(module_id.to_string());
        let metric_id = Arc::new(format!("module-runtime-static/{}", module_id));
        Self {
            module,
            root: Arc::new(profile.asset_root),
            index: Arc::new(profile.index_file),
            services_manifest: profile.services_manifest.map(Arc::new),
            diagnostics,
            metric_id,
            control_plane_base: Arc::new(control_plane_base),
            http_client: HttpClient::new(),
            log_file: Arc::new(log_file),
        }
    }

    /// Append a log line to the module's log file
    fn log_request(
        &self,
        method: &str,
        path: &str,
        status: u16,
        duration_ms: f64,
        error: Option<&str>,
    ) {
        use std::io::Write;
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string());

        let line = if let Some(err) = error {
            format!(
                "{} {} {} {} {}ms - ERROR: {}\n",
                timestamp, method, path, status, duration_ms as u64, err
            )
        } else {
            format!(
                "{} {} {} {} {}ms\n",
                timestamp, method, path, status, duration_ms as u64
            )
        };

        // Best-effort append to log file
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_file.as_ref())
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// Log an error message to the module's log file
    fn log_error(&self, message: &str) {
        use std::io::Write;
        let timestamp = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string());
        let line = format!("{} [ERROR] {}\n", timestamp, message);

        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_file.as_ref())
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn module(&self) -> &str {
        &self.module
    }

    fn index(&self) -> &Path {
        self.index.as_ref()
    }

    fn metric(&self) -> &str {
        self.metric_id.as_ref()
    }

    fn services_manifest(&self) -> Option<&Path> {
        self.services_manifest.as_deref().map(|path| path.as_path())
    }

    fn control_plane_base(&self) -> &str {
        self.control_plane_base.as_ref()
    }

    fn http_client(&self) -> &HttpClient {
        &self.http_client
    }

    fn resolve_asset(&self, request_path: &str) -> Option<PathBuf> {
        if request_path == "/" || request_path.is_empty() {
            return None;
        }
        let cleaned = sanitize_request_path(request_path);
        if cleaned.as_os_str().is_empty() {
            return None;
        }
        let candidate = self.root.join(&cleaned);
        if candidate.is_file() {
            Some(candidate)
        } else if candidate.is_dir() {
            let index = candidate.join("index.html");
            if index.is_file() {
                Some(index)
            } else {
                None
            }
        } else {
            None
        }
    }

    fn record_probe(&self, latency: Duration, success: bool) {
        let latency_ms = latency.as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe(self.metric(), latency_ms, success);
    }

    fn record_heartbeat(&self) {
        self.diagnostics.record_heartbeat(self.metric());
    }
}

/// Middleware that logs every HTTP request to the module's log file
async fn static_request_logger(
    State(state): State<StaticRouterState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let started = Instant::now();

    let response = next.run(request).await;

    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16();

    // Determine error message for non-success status codes
    let error_hint = match status {
        400..=499 => Some(match status {
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            408 => "Request Timeout",
            _ => "Client Error",
        }),
        500..=599 => Some(match status {
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Server Error",
        }),
        _ => None,
    };

    // Log to module's log file
    state.log_request(&method, &path, status, duration_ms, error_hint);

    // Also record metrics for diagnostics
    state.record_probe(started.elapsed(), response.status().is_success());

    response
}

#[derive(Serialize)]
struct StaticHealthResponse {
    status: &'static str,
    module: String,
}

async fn static_health_handler(State(state): State<StaticRouterState>) -> impl IntoResponse {
    state.record_heartbeat();
    Json(StaticHealthResponse {
        status: "ok",
        module: state.module().to_string(),
    })
}

async fn static_services_handler(State(state): State<StaticRouterState>) -> impl IntoResponse {
    state.record_heartbeat();
    if let Some(path) = state.services_manifest() {
        match fs::read_to_string(path).await {
            Ok(contents) => match serde_json::from_str::<Value>(&contents) {
                Ok(mut value) => {
                    if value.get("module_id").is_none() {
                        value["module_id"] = Value::String(state.module().to_string());
                    }
                    if value.get("services").is_none() || !value["services"].is_array() {
                        value["services"] = Value::Array(Vec::new());
                    }
                    return Json(value);
                }
                Err(err) => {
                    warn!(
                        module = %state.module(),
                        path = %path.display(),
                        error = %err,
                        "{}",
                        messages::infra::modules::runtime::process::static_services_manifest_invalid(&err)
                    );
                }
            },
            Err(err) => {
                warn!(
                    module = %state.module(),
                    path = %path.display(),
                    error = %err,
                    "{}",
                    messages::infra::modules::runtime::process::static_services_manifest_read_failed(&err)
                );
            }
        }
    }
    Json(default_static_services_payload(state.module()))
}

async fn static_gateway_proxy_handler(
    State(state): State<StaticRouterState>,
    req: Request<Body>,
) -> Response {
    let started = Instant::now();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let target = format!("{}{}", state.control_plane_base(), path_and_query);
    let method = req.method().clone();
    let headers = req.headers().clone();
    let collected_body = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            error!(
                module = %state.module(),
                error = %err,
                "failed to read gateway proxy request body"
            );
            state.record_probe(started.elapsed(), false);
            return (StatusCode::BAD_GATEWAY, "gateway proxy body read failed").into_response();
        }
    };

    let reqwest_method =
        ReqwestMethod::from_bytes(method.as_str().as_bytes()).unwrap_or(ReqwestMethod::GET);
    let mut request_builder = state.http_client().request(reqwest_method, target);
    for (name, value) in headers.iter() {
        if name == header::HOST {
            continue;
        }
        if let Ok(value_str) = value.to_str() {
            request_builder = request_builder.header(name.as_str(), value_str);
        }
    }

    if !collected_body.is_empty() {
        request_builder = request_builder.body(collected_body.to_vec());
    }

    let response_result = request_builder.send().await;
    match response_result {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            match resp.bytes().await {
                Ok(resp_bytes) => {
                    let mut builder = Response::builder().status(
                        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                    );
                    for (name, value) in headers.iter() {
                        if name.as_str().eq_ignore_ascii_case("transfer-encoding") {
                            continue;
                        }
                        if let Ok(value_str) = value.to_str() {
                            builder = builder.header(name.as_str(), value_str);
                        }
                    }
                    state.record_probe(started.elapsed(), true);
                    builder.body(Body::from(resp_bytes)).unwrap_or_else(|_| {
                        (
                            StatusCode::BAD_GATEWAY,
                            "gateway proxy response build failed",
                        )
                            .into_response()
                    })
                }
                Err(err) => {
                    warn!(
                        module = %state.module(),
                        error = %err,
                        "gateway proxy response read failed"
                    );
                    state.record_probe(started.elapsed(), false);
                    (
                        StatusCode::BAD_GATEWAY,
                        "gateway proxy upstream read failed",
                    )
                        .into_response()
                }
            }
        }
        Err(err) => {
            let error_msg = format!("Gateway proxy request failed: {}", err);
            state.log_error(&error_msg);
            warn!(
                module = %state.module(),
                error = %err,
                "gateway proxy request failed"
            );
            state.record_probe(started.elapsed(), false);
            (StatusCode::BAD_GATEWAY, "gateway proxy unavailable").into_response()
        }
    }
}

fn default_static_services_payload(module: &str) -> Value {
    json!({
        "module_id": module,
        "services": [{
            "service_id": "static",
            "name": format!("{module} static site"),
            "description": "Static assets served by the Fenrir module runtime",
            "kind": "web",
            "route_prefix": "/",
            "health_path": "/health",
            "internal_only": false,
            "ingress_access": "public",
            "protocols": ["http"],
            "tags": ["static-site"]
        }]
    })
}

async fn static_asset_handler(State(state): State<StaticRouterState>, uri: Uri) -> Response {
    let started = Instant::now();
    let mut success = false;
    let response = if let Some(path) = state.resolve_asset(uri.path()) {
        match fs::read(&path).await {
            Ok(bytes) => {
                success = true;
                build_static_response(bytes, &path)
            }
            Err(err) => {
                let error_msg = format!("Failed to read asset '{}': {}", path.display(), err);
                state.log_error(&error_msg);
                warn!(
                    module = %state.module(),
                    path = %path.display(),
                    error = %err,
                    "failed to read static module asset"
                );
                (StatusCode::INTERNAL_SERVER_ERROR, "failed to read asset").into_response()
            }
        }
    } else {
        match fs::read(state.index()).await {
            Ok(bytes) => {
                success = true;
                build_static_response(bytes, state.index())
            }
            Err(err) => {
                let error_msg = format!("index.html missing or unreadable: {}", err);
                state.log_error(&error_msg);
                error!(
                    module = %state.module(),
                    error = %err,
                    "static module index.html missing"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "module assets missing index.html",
                )
                    .into_response()
            }
        }
    };
    state.record_probe(started.elapsed(), success);
    response
}

fn sanitize_request_path(path: &str) -> PathBuf {
    let mut buf = PathBuf::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        buf.push(segment);
    }
    buf
}

fn build_static_response(bytes: Vec<u8>, path: &Path) -> Response {
    let content_type = content_type_for(path);
    let cache_control = if content_type.starts_with("text/html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(bytes))
        .unwrap_or_else(|err| {
            error!(error = %err, "failed to build static asset response");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("internal server error"))
                .unwrap()
        })
}

fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/infra/modules/runtime/process_tests.rs"]
mod tests;
