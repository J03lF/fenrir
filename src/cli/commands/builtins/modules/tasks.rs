use std::io::{self, Write};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::audit::{AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{CliDependencies, CommandOutcome, ConfirmationHandler};
use crate::domain::module::{
    ModuleId, ModuleInstallStatus, ModuleProgress, ModuleRuntimeError, ModuleStartConfig,
    ModuleVersion,
};
use crate::services::module::{
    DistributionAction, DistributionPlanEntry, ModuleService, ModuleSyncOutcome,
};
use crate::utils::messages::cli::builtins::modules as msg_modules;

use super::ctx::{
    dev_services_metadata, module_error_metadata, record_module_audit, run_module_runtime_call,
    OwnedStreamedWriter,
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
            record_module_audit(
                &deps,
                "module::synchronize",
                &module_id,
                Some(&dev_services.version),
                AuditOutcome::Success,
                dev_services_metadata(&dev_services),
            );
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
        Ok(install_result) => {
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
            let mut steps: Vec<(String, String)> = Vec::new();

            if matches!(entry.action, DistributionAction::Update) {
                let module_id_for_status = entry.module_id.clone();
                match run_module_runtime_call(Arc::clone(&service), move |svc| async move {
                    svc.runtime_status(&module_id_for_status).await
                }) {
                    Ok(_) => {
                        let module_id_for_stop = entry.module_id.clone();
                        if let Err(err) =
                            run_module_runtime_call(Arc::clone(&service), move |svc| async move {
                                svc.stop(&module_id_for_stop).await
                            })
                        {
                            render_runtime_error(
                                &mut out,
                                msg_modules::tasks_flow::runtime_contexts::STOP_FAILED,
                                &err,
                            )?;
                            return Ok(CommandOutcome::Continue);
                        }
                        steps.push((
                            msg_modules::tasks_flow::steps::STOPPING.to_string(),
                            msg_modules::tasks_flow::details::STOPPED_OK.to_string(),
                        ));
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
            let mut download_bar: Option<String> = None;
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
                                download_bar = Some(format!(
                                    "{} {:>3}% ({:.1} MB / {:.1} MB)",
                                    bar, percent, mb_downloaded, mb_total
                                ));
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
                    if let Some(bar) = download_bar.take() {
                        steps.push((msg_modules::tasks_flow::steps::DOWNLOADING.to_string(), bar));
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
                    steps.push((
                        msg_modules::tasks_flow::steps::INSTALLING.to_string(),
                        detail,
                    ));
                }
                Err(err) => {
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
                let module_id_for_start = entry.module_id.clone();
                let start_result =
                    run_module_runtime_call(Arc::clone(&service), move |svc| async move {
                        let config = ModuleStartConfig {
                            module_id: module_id_for_start,
                            port: None,
                            env_vars: Vec::new(),
                            auto_restart: true,
                        };
                        svc.start(config).await
                    });
                match start_result {
                    Ok(info) => {
                        let pid_hint = msg_modules::tasks_flow::details::start_running(info.pid);
                        steps.push((
                            msg_modules::tasks_flow::steps::STARTING.to_string(),
                            pid_hint,
                        ));
                    }
                    Err(ModuleRuntimeError::AlreadyRunning { .. }) => steps.push((
                        msg_modules::tasks_flow::steps::STARTING.to_string(),
                        msg_modules::tasks_flow::details::START_ALREADY_RUNNING.to_string(),
                    )),
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
                steps.push((
                    msg_modules::tasks_flow::steps::STARTING.to_string(),
                    msg_modules::tasks_flow::details::START_NOT_REQUIRED.to_string(),
                ));
            }

            render_task_block(&mut out, &module_label, &steps)?;
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
