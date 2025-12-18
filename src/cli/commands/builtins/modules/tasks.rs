use std::fs;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::Arc;

use time::OffsetDateTime;
use tokio::sync::mpsc;

use crate::audit::{AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{CliDependencies, CommandOutcome, ConfirmationHandler};
use crate::domain::module::{
    ModuleId, ModuleInstallStatus, ModuleProgress, ModuleRuntimeError, ModuleStartConfig,
    ModuleVersion,
};
use crate::services::module::{
    sanitize_service_id, write_plain_env_file, write_shell_env_file, DevEnvArtifacts,
    DistributionAction, DistributionPlanEntry, ModuleDevServices, ModuleService, ModuleSyncOutcome,
};
use crate::utils::messages::cli::builtins::modules as msg_modules;

use super::ctx::{
    dev_services_metadata, module_error_metadata, record_module_audit, OwnedStreamedWriter,
};
use super::output::{module_error_code, render_runtime_error, render_service_error};

pub(super) fn render_task_block(
    out: &mut dyn Write,
    module_label: &str,
    steps: &[(String, String)],
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        msg_modules::tasks_flow::block_header(module_label)
    )?;
    if steps.is_empty() {
        return Ok(());
    }
    for (idx, (label, detail)) in steps.iter().enumerate() {
        let is_last = idx + 1 == steps.len();
        let prefix = if is_last { "└──" } else { "├──" };
        writeln!(out, "         {} {:<14} {}", prefix, label, detail)?;
    }
    out.flush()
}

struct LiveTaskBlock<W: Write> {
    out: W,
    progress_prefix: Option<String>,
}

impl<W: Write> LiveTaskBlock<W> {
    fn new(mut out: W, module_label: &str) -> io::Result<Self> {
        writeln!(
            out,
            "{}",
            msg_modules::tasks_flow::block_header(module_label)
        )?;
        Ok(Self {
            out,
            progress_prefix: None,
        })
    }

    fn step(&mut self, label: &str, detail: &str) -> io::Result<()> {
        self.print_line("├──", label, detail)
    }

    fn final_step(&mut self, label: &str, detail: &str) -> io::Result<()> {
        self.print_line("└──", label, detail)
    }

    fn begin_progress(&mut self, label: &str, detail: &str) -> io::Result<()> {
        let prefix = format!("         {} {:<14} ", "├──", label);
        write!(self.out, "{}{}", prefix, detail)?;
        self.out.flush()?;
        self.progress_prefix = Some(prefix);
        Ok(())
    }

    fn update_progress(&mut self, detail: &str) -> io::Result<()> {
        if let Some(prefix) = &self.progress_prefix {
            write!(self.out, "\r{}{}\x1b[K", prefix, detail)?;
            self.out.flush()?;
        }
        Ok(())
    }

    fn finish_progress(&mut self, detail: &str) -> io::Result<()> {
        if let Some(prefix) = self.progress_prefix.take() {
            writeln!(self.out, "\r{}{}", prefix, detail)?;
            self.out.flush()?;
        }
        Ok(())
    }

    fn cancel_progress(&mut self) -> io::Result<()> {
        if self.progress_prefix.take().is_some() {
            writeln!(self.out)?;
            self.out.flush()?;
        }
        Ok(())
    }

    fn print_line(&mut self, prefix: &str, label: &str, detail: &str) -> io::Result<()> {
        writeln!(self.out, "         {} {:<14} {}", prefix, label, detail)?;
        self.out.flush()
    }
}

pub(super) async fn run_synchronize_task(
    deps: CliDependencies,
    service: Arc<ModuleService>,
    module_id: ModuleId,
) -> io::Result<CommandOutcome> {
    let sink = deps
        .output()
        .ok_or_else(|| io::Error::other("no output sink available"))?;
    let mut out = OwnedStreamedWriter::new(sink);

    writeln!(
        &mut out,
        "{}",
        msg_modules::tasks_flow::sync_header(&module_id.to_string())
    )?;
    let result = service.synchronize_from_local(&module_id).await;
    match result {
        Ok(ModuleSyncOutcome::Packaged(package)) => {
            let package = *package;
            let version = ModuleVersion(package.install_result.manifest.version.clone());
            let steps = vec![
                (
                    msg_modules::tasks_flow::steps::PACKAGING.to_string(),
                    msg_modules::tasks_flow::details::packaging_ok(
                        &package.packaged_from.display().to_string(),
                    ),
                ),
                (
                    msg_modules::tasks_flow::steps::INSTALLING.to_string(),
                    msg_modules::tasks_flow::details::install_version(&version.to_string()),
                ),
                (
                    msg_modules::tasks_flow::steps::STARTING.to_string(),
                    msg_modules::tasks_flow::details::START_AUTO_MANAGED.to_string(),
                ),
            ];
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;
            writeln!(
                out,
                "{}",
                msg_modules::tasks_flow::sync_done(&module_id.to_string())
            )?;
            record_module_audit(
                &deps,
                "module::synchronize",
                &module_id,
                Some(&version),
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("mode", "package")
                    .insert("packaged_from", package.packaged_from.display().to_string()),
            );
        }
        Ok(ModuleSyncOutcome::ExternalServices(dev_services)) => {
            let steps = vec![(
                msg_modules::tasks_flow::steps::SWITCHING.to_string(),
                msg_modules::tasks_flow::details::SWITCH_DEV_SERVICES.to_string(),
            )];
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;
            writeln!(
                out,
                "{}",
                msg_modules::tasks_flow::sync_done(&module_id.to_string())
            )?;
            let env_artifacts = match persist_dev_env_files(
                Arc::clone(&service),
                &module_id,
                &dev_services,
                &mut out,
            )
            .await
            {
                Ok(artifacts) => artifacts,
                Err(err) => {
                    writeln!(
                        out,
                        "{}",
                        msg_modules::synchronize_flow::env_file_error(&err.to_string())
                    )?;
                    DevEnvArtifacts::default()
                }
            };
            if let Some(run) = &dev_services.run {
                if run.auto_start {
                    if let Some(log_path) = &run.log_path {
                        writeln!(
                            out,
                            "{}",
                            msg_modules::synchronize_flow::dev_agent_started(
                                &run.command,
                                &run.workdir.display().to_string(),
                                &log_path.display().to_string(),
                                run.auto_restart,
                            )
                        )?;
                    }
                } else {
                    let hint_path = env_artifacts
                        .shared_env
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .or_else(|| {
                            env_artifacts
                                .per_service
                                .first()
                                .map(|(_, path)| path.display().to_string())
                        });
                    writeln!(
                        out,
                        "{}",
                        msg_modules::synchronize_flow::dev_agent_manual(
                            &run.command,
                            &run.workdir.display().to_string(),
                            hint_path.as_deref(),
                        )
                    )?;
                    if let (Some(shared_path), Some(expires_at)) = (
                        env_artifacts.shared_env.clone(),
                        env_artifacts.shared_expires_at,
                    ) {
                        if let Err(err) = service
                            .enable_manual_token_rotation(
                                &module_id,
                                shared_path,
                                env_artifacts.shared_endpoint,
                                expires_at,
                            )
                            .await
                        {
                            writeln!(out, "warning: failed to enable token rotation ({err})")?;
                        }
                    }
                }
            } else {
                writeln!(
                    out,
                    "{}",
                    msg_modules::synchronize_flow::DEV_AGENT_NOT_CONFIGURED
                )?;
            }
            record_module_audit(
                &deps,
                "module::synchronize",
                &module_id,
                Some(&dev_services.version),
                AuditOutcome::Success,
                dev_services_metadata(&dev_services),
            );
            if let Some(run) = &dev_services.run {
                if run.auto_start {
                    let mut metadata = AuditMetadata::default()
                        .insert("command", run.command.clone())
                        .insert("workdir", run.workdir.display().to_string())
                        .insert("auto_restart", run.auto_restart.to_string());
                    if let Some(log_path) = &run.log_path {
                        metadata = metadata.insert("log_path", log_path.display().to_string());
                    }
                    record_module_audit(
                        &deps,
                        "module::sync::dev-agent-start",
                        &module_id,
                        Some(&dev_services.version),
                        AuditOutcome::Success,
                        metadata,
                    );
                }
            }
        }
        Err(err) => {
            render_service_error(&mut out, msg_modules::synchronize_flow::ERROR_CONTEXT, &err)?;
            record_module_audit(
                &deps,
                "module::synchronize",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(module_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

async fn persist_dev_env_files(
    service: Arc<ModuleService>,
    module_id: &ModuleId,
    dev_services: &ModuleDevServices,
    out: &mut OwnedStreamedWriter,
) -> io::Result<DevEnvArtifacts> {
    let Some(module_root) = service.dev_module_root(module_id) else {
        return Ok(DevEnvArtifacts::default());
    };
    let env_dir = module_root.join(".fenrir");
    fs::create_dir_all(&env_dir)?;
    let shared_env_path = env_dir.join("dev.env");
    let shared_env_display = shared_env_path.display().to_string();

    let mut files = Vec::new();
    let mut shared_env_values: Option<Vec<(String, String)>> = None;
    let mut shared_endpoint: Option<SocketAddr> = None;
    let mut shared_expires_at: Option<OffsetDateTime> = None;
    for svc in &dev_services.services {
        let env_export = service
            .export_runtime_environment(module_id, Some(svc.endpoint))
            .await
            .map_err(|err| io::Error::other(err.to_string()))?;
        let mut env_values = env_export.entries.clone();
        env_values.push((
            "FENRIR_DEV_ENV_FILE".to_string(),
            shared_env_display.clone(),
        ));
        if shared_env_values.is_none() {
            shared_env_values = Some(env_values.clone());
            shared_endpoint = Some(svc.endpoint);
            shared_expires_at = Some(env_export.token.claims.expires_at);
        }
        let service_suffix = svc.service_id.split("::").last().unwrap_or(&svc.service_id);
        let file_name = format!("dev-env-{}.sh", sanitize_service_id(service_suffix));
        let file_path = env_dir.join(file_name);
        write_shell_env_file(&file_path, &env_values)?;
        writeln!(
            out,
            "{}",
            msg_modules::synchronize_flow::env_file_hint(
                &svc.service_id,
                &file_path.display().to_string()
            )
        )?;
        files.push((svc.service_id.clone(), file_path));
    }

    let shared_env = if let Some(values) = shared_env_values {
        write_plain_env_file(&shared_env_path, &values)?;
        Some(shared_env_path)
    } else {
        None
    };

    Ok(DevEnvArtifacts {
        per_service: files,
        shared_env,
        shared_endpoint,
        shared_expires_at,
    })
}

pub(super) async fn run_release_task(
    deps: CliDependencies,
    service: Arc<ModuleService>,
    module_id: ModuleId,
    fenrir_version: String,
) -> io::Result<CommandOutcome> {
    let sink = deps
        .output()
        .ok_or_else(|| io::Error::other("no output sink available"))?;
    let mut out = OwnedStreamedWriter::new(sink);

    writeln!(
        &mut out,
        "{}",
        msg_modules::tasks_flow::release_header(&module_id.to_string())
    )?;
    let fenrir_version_for_call = fenrir_version.clone();
    let result = service
        .release_override(&module_id, &fenrir_version_for_call)
        .await;

    match result {
        Ok(outcome) => {
            let install_result = outcome.install_result;
            let recorded_version = ModuleVersion(install_result.manifest.version.clone());
            let steps = vec![
                (
                    msg_modules::tasks_flow::steps::RESETTING.to_string(),
                    msg_modules::tasks_flow::details::RESET_OVERRIDES.to_string(),
                ),
                (
                    msg_modules::tasks_flow::steps::INSTALLING.to_string(),
                    msg_modules::tasks_flow::details::install_version(
                        &recorded_version.to_string(),
                    ),
                ),
                (
                    msg_modules::tasks_flow::steps::STARTING.to_string(),
                    msg_modules::tasks_flow::details::START_AUTO_MANAGED.to_string(),
                ),
            ];
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;
            writeln!(
                &mut out,
                "{}",
                msg_modules::tasks_flow::release_done(
                    &module_id.to_string(),
                    &recorded_version.to_string()
                )
            )?;
            record_module_audit(
                &deps,
                "module::release",
                &module_id,
                Some(&recorded_version),
                AuditOutcome::Success,
                AuditMetadata::default().insert("source", install_result.source.label()),
            );
            if outcome.dev_override_cleared {
                record_module_audit(
                    &deps,
                    "module::sync::dev-agent-stop",
                    &module_id,
                    Some(&recorded_version),
                    AuditOutcome::Success,
                    AuditMetadata::default().insert("reason", "release"),
                );
            }
        }
        Err(err) => {
            render_service_error(&mut out, msg_modules::release_flow::ERROR_CONTEXT, &err)?;
            record_module_audit(
                &deps,
                "module::release",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(module_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

pub(super) struct InstallDistributionConfirmation {
    pub(super) service: Arc<ModuleService>,
    pub(super) plan: Vec<DistributionPlanEntry>,
    pub(super) fenrir_version: String,
}

impl ConfirmationHandler for InstallDistributionConfirmation {
    fn handle(
        self: Box<Self>,
        accepted: bool,
        deps: &CliDependencies,
        out: &mut dyn Write,
    ) -> io::Result<CommandOutcome> {
        if !accepted {
            writeln!(out, "{}", msg_modules::tasks_flow::INSTALL_ABORTED)?;
            return Ok(CommandOutcome::Continue);
        }

        let deps_clone = deps.clone();
        let future = async move { self.execute(&deps_clone).await };
        Ok(CommandOutcome::AsyncTask(Box::pin(future)))
    }
}

impl InstallDistributionConfirmation {
    async fn execute(self, deps: &CliDependencies) -> io::Result<CommandOutcome> {
        let sink = deps
            .output()
            .ok_or_else(|| io::Error::other("no output sink available"))?;
        let mut out = OwnedStreamedWriter::new(sink);

        writeln!(&mut out)?;

        let actionable: Vec<_> = self
            .plan
            .into_iter()
            .filter(|entry| entry.action.requires_execution())
            .collect();

        if actionable.is_empty() {
            writeln!(
                out,
                "{}",
                msg_modules::tasks_flow::fenrir_already_current(&self.fenrir_version)
            )?;
            return Ok(CommandOutcome::Continue);
        }

        let service = Arc::clone(&self.service);

        for entry in actionable.iter() {
            let module_label = entry.module_id.to_string();
            let mut needs_restart = matches!(entry.action, DistributionAction::Install);
            let mut block = LiveTaskBlock::new(&mut out, &module_label)?;

            if matches!(entry.action, DistributionAction::Update) {
                // Use direct async calls instead of run_module_runtime_call to avoid blocking
                match service.runtime_status(&entry.module_id).await {
                    Ok(_) => {
                        if let Err(err) = service.stop(&entry.module_id).await {
                            render_runtime_error(
                                &mut out,
                                msg_modules::tasks_flow::runtime_contexts::STOP_FAILED,
                                &err,
                            )?;
                            return Ok(CommandOutcome::Continue);
                        }
                        block.step(
                            msg_modules::tasks_flow::steps::STOPPING,
                            msg_modules::tasks_flow::details::STOPPED_OK,
                        )?;
                        needs_restart = true;
                    }
                    Err(ModuleRuntimeError::NotRunning { .. }) => {}
                    Err(err) => {
                        render_runtime_error(
                            &mut out,
                            msg_modules::tasks_flow::runtime_contexts::STATUS_UNAVAILABLE,
                            &err,
                        )?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            }

            let module_id_for_install = entry.module_id.clone();
            let target_version = entry.target_version.clone();
            let service_for_install = Arc::clone(&service);

            let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ModuleProgress>();
            let progress_callback: Arc<dyn Fn(ModuleProgress) + Send + Sync> =
                Arc::new(move |progress: ModuleProgress| {
                    let _ = progress_tx.send(progress);
                });

            let install_task = tokio::spawn(async move {
                service_for_install
                    .install_with_progress(
                        &module_id_for_install,
                        Some(&target_version),
                        Some(progress_callback),
                    )
                    .await
            });

            let mut last_percent = 0u32;
            let mut download_detail: Option<String> = None;
            let mut download_started = false;
            let mut install_task = install_task;
            let install_result = loop {
                tokio::select! {
                    biased;
                    Some(progress) = progress_rx.recv() => {
                        if let ModuleProgress::DownloadProgress {
                            downloaded_bytes,
                            total_bytes: Some(total),
                            ..
                        } = progress
                        {
                            let percent =
                                ((downloaded_bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
                                    as u32;
                            if percent >= last_percent + 5 || percent == 100 {
                                let mb_downloaded = downloaded_bytes as f64 / 1_048_576.0;
                                let mb_total = total as f64 / 1_048_576.0;
                                let bar_width = 20;
                                let filled =
                                    ((percent as f64 / 100.0) * bar_width as f64).round() as usize;
                                let filled = filled.min(bar_width);
                                let bar = format!(
                                    "[{}{}]",
                                    "=".repeat(filled),
                                    ".".repeat(bar_width.saturating_sub(filled))
                                );
                                let detail = format!(
                                    "{} {:>3}% ({:.1} MB / {:.1} MB)",
                                    bar, percent, mb_downloaded, mb_total
                                );
                                download_detail = Some(detail.clone());
                                if !download_started {
                                    block.begin_progress(
                                        msg_modules::tasks_flow::steps::DOWNLOADING,
                                        &detail,
                                    )?;
                                    download_started = true;
                                } else {
                                    block.update_progress(&detail)?;
                                }
                                last_percent = percent;
                            }
                        }
                    }
                    result = &mut install_task => {
                        let result = result.map_err(|err| {
                            io::Error::other(format!("install task panicked: {err:?}"))
                        })?;
                        break result;
                    }
                }
            };

            match install_result {
                Ok(result) => {
                    if download_started {
                        let fallback;
                        let detail = if let Some(text) = download_detail.as_deref() {
                            text
                        } else {
                            fallback = msg_modules::tasks_flow::details::install_version("latest");
                            fallback.as_str()
                        };
                        block.finish_progress(detail)?;
                    } else if let Some(detail) = download_detail.as_deref() {
                        block.step(msg_modules::tasks_flow::steps::DOWNLOADING, detail)?;
                    }
                    let version_str = result.manifest.version.to_string();
                    let detail = match result.status {
                        ModuleInstallStatus::Installed => {
                            msg_modules::tasks_flow::details::install_version(&version_str)
                        }
                        ModuleInstallStatus::Updated => {
                            msg_modules::tasks_flow::details::update_version(&version_str)
                        }
                        ModuleInstallStatus::AlreadyCurrent => {
                            needs_restart = false;
                            msg_modules::tasks_flow::details::current_version(&version_str)
                        }
                    };
                    block.step(msg_modules::tasks_flow::steps::INSTALLING, &detail)?;
                }
                Err(err) => {
                    block.cancel_progress()?;
                    render_service_error(
                        &mut out,
                        &msg_modules::tasks_flow::service_contexts::installation_failed(
                            &module_label,
                        ),
                        &err,
                    )?;
                    return Ok(CommandOutcome::Continue);
                }
            }

            if needs_restart {
                // Use direct async call instead of run_module_runtime_call to avoid blocking
                        let config = ModuleStartConfig {
                    module_id: entry.module_id.clone(),
                            port: None,
                            env_vars: Vec::new(),
                            auto_restart: true,
                        };
                match service.start(config).await {
                    Ok(info) => {
                        let pid_hint = msg_modules::tasks_flow::details::start_running(info.pid);
                        block.final_step(msg_modules::tasks_flow::steps::STARTING, &pid_hint)?;
                    }
                    Err(ModuleRuntimeError::AlreadyRunning { .. }) => block.final_step(
                        msg_modules::tasks_flow::steps::STARTING,
                        msg_modules::tasks_flow::details::START_ALREADY_RUNNING,
                    )?,
                    Err(err) => {
                        render_runtime_error(
                            &mut out,
                            msg_modules::tasks_flow::runtime_contexts::START_FAILED,
                            &err,
                        )?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            } else {
                block.final_step(
                    msg_modules::tasks_flow::steps::STARTING,
                    msg_modules::tasks_flow::details::START_NOT_REQUIRED,
                )?;
            }
            writeln!(&mut out)?;
        }

        writeln!(out)?;
        writeln!(
            out,
            "{}",
            msg_modules::tasks_flow::distribution_applied(&self.fenrir_version)
        )?;
        out.flush()?;

        Ok(CommandOutcome::Continue)
    }
}
