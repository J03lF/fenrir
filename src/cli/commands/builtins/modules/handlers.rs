use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use tracing::{info, warn};

use crate::audit::{AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ConfirmationRequest, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::cli::output::BoxTable;
use crate::config;
use crate::domain::module::{ModuleId, ModuleInstallSource, ModuleRuntimeInfo, ModuleVersion};
use crate::services::module::{
    DistributionAction, ModuleCanaryRoutingStatus, ModuleRollingRestartReport,
    ModuleRuntimeInstanceSnapshot, ModuleScaffoldOptions, ModuleScaffoldRuntime, ModuleService,
    ModuleSyncOutcome,
};
use crate::services::{ServiceSnapshot, ServiceStatus};
use crate::utils;
use crate::utils::messages::cli::builtins::modules as msg_modules;

use super::ctx::{dev_services_metadata, module_error_metadata, ModulesCommandCtx};
use super::entries::{available_subcommands, resolve_module_subcommand};
use super::output::{
    module_error_code, render_distribution_plan, render_manifest, render_runtime_error,
    render_scaffold_summary, render_service_error, runtime_error_code,
};
use super::tasks::{run_release_task, run_synchronize_task, InstallDistributionConfirmation};

pub(super) fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    info!(command = "modules", "modules command invoked");
    let Some(service) = module_service_or_notify(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };

    let ctx = ModulesCommandCtx::new(deps, service);
    let (subcommand_name, rest_args) = match args.split_first() {
        Some((name, rest)) => (*name, rest),
        None => ("list", &[][..]),
    };

    if let Some(canonical) = resolve_module_subcommand(subcommand_name) {
        dispatch_module(canonical, &ctx, out, rest_args)?;
    } else if subcommand_name.is_empty() {
        handle_list(&ctx, out, rest_args)?;
    } else {
        writeln!(
            out,
            "{}",
            msg_modules::routing::unknown_subcommand(subcommand_name)
        )?;
        writeln!(
            out,
            "{}",
            msg_modules::routing::available_subcommands(&available_subcommands())
        )?;
    }

    Ok(CommandOutcome::Continue)
}

pub(super) fn handle_search_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &SEARCH_SPEC)
}

pub(super) fn handle_install_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &INSTALL_DISTRIBUTION_SPEC)
}

pub(super) fn handle_synchronize_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &SYNCHRONIZE_SPEC)
}

pub(super) fn handle_release_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &RELEASE_SPEC)
}

pub(super) fn handle_scaffold_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &SCAFFOLD_SPEC)
}

pub(super) fn handle_uninstall_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &UNINSTALL_SPEC)
}

pub(super) fn handle_check_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &CHECK_SPEC)
}

pub fn run_module_command(
    deps: &CliDependencies,
    action: &str,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some(service) = module_service_or_notify(deps, out)? else {
        return Ok(CommandOutcome::Continue);
    };
    let ctx = ModulesCommandCtx::new(deps, service);
    dispatch_module(action, &ctx, out, args)
}

fn dispatch_module(
    canonical: &str,
    ctx: &ModulesCommandCtx<'_>,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    match canonical {
        "list" => handle_list(ctx, out, args),
        "search" => handle_search(ctx, out, args),
        "info" => handle_info(ctx, out, args),
        "install-distribution" => handle_install_distribution(ctx, out, args),
        "synchronize" => handle_synchronize(ctx, out, args),
        "release" => handle_release(ctx, out, args),
        "scaffold" => handle_scaffold(ctx, out, args),
        "release-dev-overrides" => handle_release_dev_overrides(ctx, out, args),
        "uninstall" => handle_uninstall(ctx, out, args),
        "check-updates" => handle_check_updates(ctx, out, args),
        "log" => handle_logs(ctx, out, args),
        "env" => handle_env(ctx, out, args),
        "services" => handle_services(ctx, out, args),
        "start" => handle_start(ctx, out, args),
        "stop" => handle_stop(ctx, out, args),
        "restart" => handle_restart(ctx, out, args),
        "instances" => handle_instances(ctx, out, args),
        "rolling-restart" => handle_rolling_restart(ctx, out, args),
        "canary" => handle_canary(ctx, out, args),
        "reload-overrides" => handle_reload_overrides(ctx, out, args),
        "stop-all" => handle_stop_all(ctx, out, args),
        other => {
            warn!(
                target = "cli::modules",
                subcommand = other,
                "missing module handler mapping"
            );
            writeln!(out, "{}", msg_modules::routing::unimplemented(other))?;
            Ok(CommandOutcome::Continue)
        }
    }
}

fn handle_list(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if !args.is_empty() {
        writeln!(out, "{}", msg_modules::list_modules::EXTRA_ARGS_WARNING)?;
    }

    let modules = match ctx.module_call(|service| async move { service.list_installed().await }) {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(out, msg_modules::list_modules::LOAD_ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if modules.is_empty() {
        writeln!(out, "{}", msg_modules::list_modules::TITLE)?;
        writeln!(out, "{}", msg_modules::list_modules::UNDERLINE)?;
        writeln!(out, "{}", msg_modules::list_modules::EMPTY_STATE)?;
        return Ok(CommandOutcome::Continue);
    }

    let dev_override_modules: HashSet<String> =
        match ctx.module_call(|service| async move { Ok(service.dev_override_modules().await) }) {
            Ok(ids) => ids.into_iter().map(|id| id.to_string()).collect(),
            Err(err) => {
                warn!(
                    target = "cli::modules",
                    error = %err,
                    "failed to read dev override state"
                );
                HashSet::new()
            }
        };

    let runtime_infos: HashMap<_, _> =
        match ctx.runtime_call(|service| async move { service.list_running().await }) {
            Ok(infos) => infos
                .into_iter()
                .map(|info| (info.module_id.to_string(), info))
                .collect(),
            Err(_) => HashMap::new(),
        };

    let mut table = BoxTable::new(
        msg_modules::list_modules::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    )
    .with_title(format!("Modules ({})", modules.len()));

    for module in &modules {
        let module_id = module.manifest.id.clone();
        let dev_override_active = dev_override_modules.contains(&module_id);
        let mut source_label = module.source.label().to_string();
        let mut status_label;
        let mut pid_label = msg_modules::list_modules::EMPTY_VALUE.to_string();
        let mut port_label = msg_modules::list_modules::EMPTY_VALUE.to_string();
        let mut uptime_label = msg_modules::list_modules::EMPTY_VALUE.to_string();

        if let Some(runtime_info) = runtime_infos.get(&module_id) {
            let duration = runtime_info
                .started_at
                .and_then(|started| started.elapsed().ok())
                .map(utils::format_brief_duration)
                .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string());

            status_label = msg_modules::list_modules::STATUS_RUNNING.to_string();
            pid_label = runtime_info
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string());
            port_label = runtime_info
                .port
                .map(|p| p.to_string())
                .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string());
            uptime_label = duration;
        } else {
            status_label = msg_modules::list_modules::STATUS_STOPPED.to_string();
        }

        if dev_override_active && !module.source.is_synchronized() {
            source_label = ModuleInstallSource::LocalOverride.label().to_string();
        }

        if module.source.is_synchronized() || dev_override_active {
            status_label = format!(
                "{} {}",
                status_label,
                msg_modules::list_modules::STATUS_SYNC_SUFFIX
            );
        }

        table.add_row(vec![
            module_id,
            module.manifest.version.to_string(),
            source_label,
            status_label,
            pid_label,
            port_label,
            uptime_label,
        ]);
    }

    table.render(out)?;
    Ok(CommandOutcome::Continue)
}

fn handle_search(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    use crate::domain::module::ModuleSearchQuery;

    let pattern = args.first().map(|value| (*value).to_string());
    let pattern_for_query = pattern.clone();

    let modules = match ctx.module_call(|service| async move {
        let query = ModuleSearchQuery::new(pattern_for_query);
        service.search(query).await
    }) {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(out, msg_modules::search_results::LOAD_ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    writeln!(out, "{}", msg_modules::search_results::TITLE)?;
    writeln!(out, "{}", msg_modules::search_results::UNDERLINE)?;

    if modules.is_empty() {
        writeln!(out, "{}", msg_modules::search_results::EMPTY_STATE)?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(
        msg_modules::search_results::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for entry in modules {
        table.add_row(vec![
            entry.id.to_string(),
            entry.version.to_string(),
            entry
                .description
                .unwrap_or_else(|| msg_modules::search_results::EMPTY_DESCRIPTION.to_string()),
        ]);
    }

    table.render(out, "  ")?;
    Ok(CommandOutcome::Continue)
}

fn handle_info(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let Some(target) = args.first() else {
        writeln!(out, "{}", msg_modules::info_view::USAGE)?;
        return Ok(CommandOutcome::Continue);
    };

    let (module_id, version) = match parse_module_target(target, &args[1..]) {
        Ok(tuple) => tuple,
        Err(err) => {
            writeln!(out, "{err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };

    let module_id_for_call = module_id.clone();
    let version_for_call = version.clone();

    let manifest = match ctx.module_call(|service| async move {
        service
            .manifest(&module_id_for_call, version_for_call.as_ref())
            .await
    }) {
        Ok(manifest) => manifest,
        Err(err) => {
            render_service_error(out, msg_modules::info_view::LOAD_ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    render_manifest(out, &manifest)?;
    Ok(CommandOutcome::Continue)
}

fn handle_install_distribution(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if !args.is_empty() {
        writeln!(
            out,
            "{}",
            msg_modules::install_flow::args_not_required(&ctx.deps().config.app.version)
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let fenrir_version = ctx.deps().config.app.version.clone();

    let version_for_call = fenrir_version.clone();
    let plan = match ctx
        .module_call(|service| async move { service.distribution_plan(&version_for_call).await })
    {
        Ok(plan) => plan,
        Err(err) => {
            render_service_error(out, msg_modules::install_flow::LOAD_ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if plan.is_empty() {
        writeln!(
            out,
            "{}",
            msg_modules::install_flow::not_found(&fenrir_version)
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let installed_modules =
        match ctx.module_call(|service| async move { service.list_installed().await }) {
            Ok(modules) => modules,
            Err(err) => {
                render_service_error(out, msg_modules::list_modules::LOAD_ERROR_CONTEXT, &err)?;
                return Ok(CommandOutcome::Continue);
            }
        };

    let mut synchronized: HashSet<String> = installed_modules
        .into_iter()
        .filter(|module| module.source.is_synchronized())
        .filter_map(|module| module.manifest.module_id().ok())
        .map(|id| id.to_string())
        .collect();

    let dev_override_modules: HashSet<String> =
        match ctx.module_call(|service| async move { Ok(service.dev_override_modules().await) }) {
            Ok(ids) => ids.into_iter().map(|id| id.to_string()).collect(),
            Err(err) => {
                warn!(
                    target = "cli::modules",
                    error = %err,
                    "failed to read dev override state"
                );
                HashSet::new()
            }
        };
    synchronized.extend(dev_override_modules);

    render_distribution_plan(out, &plan, &synchronized)?;

    let actionable: Vec<_> = plan
        .iter()
        .filter(|entry| {
            entry.action.requires_execution()
                && !(synchronized.contains(entry.module_id.as_str())
                    && matches!(entry.action, DistributionAction::Update))
        })
        .cloned()
        .collect();

    if actionable.is_empty() {
        writeln!(
            out,
            "{}",
            msg_modules::install_flow::already_installed(&fenrir_version)
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let confirmation = ConfirmationRequest::new(
        msg_modules::install_flow::CONFIRM_PROMPT,
        Box::new(InstallDistributionConfirmation {
            service: ctx.service(),
            plan: actionable,
            fenrir_version,
        }),
    )
    .with_command_context("install distribution", args);

    Ok(CommandOutcome::AwaitConfirmation(confirmation))
}

fn handle_uninstall(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::uninstall_flow::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };

    let module_id_for_call = module_id.clone();
    let result =
        ctx.module_call(|service| async move { service.uninstall(&module_id_for_call).await });

    match result {
        Ok(_) => {
            let module_label = module_id.to_string();
            writeln!(
                out,
                "{}",
                msg_modules::uninstall_flow::removal_header(&module_label)
            )?;
            writeln!(out, "{}", msg_modules::uninstall_flow::STOPPING_HOOKS)?;
            writeln!(out, "{}", msg_modules::uninstall_flow::UNLINKING_FILES)?;
            writeln!(out, "{}", msg_modules::uninstall_flow::CLEANUP_CONFIGS)?;
            writeln!(out)?;
            writeln!(
                out,
                "{}",
                msg_modules::uninstall_flow::removal_summary(&module_label)
            )?;
            writeln!(out)?;
            out.flush()?;
            ctx.record_audit(
                "module::uninstall",
                &module_id,
                None,
                AuditOutcome::Success,
                AuditMetadata::default(),
            );
        }
        Err(err) => {
            render_service_error(out, msg_modules::uninstall_flow::ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::uninstall",
                &module_id,
                None,
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", module_error_code(&err))
                    .insert("error", err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_synchronize(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::synchronize_flow::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };

    if ctx.output_sink().is_some() {
        let deps = ctx.deps_clone();
        let service = ctx.service();
        let module_id_for_task = module_id.clone();
        let future = async move { run_synchronize_task(deps, service, module_id_for_task).await };
        return Ok(CommandOutcome::AsyncTask(Box::pin(future)));
    }

    let module_id_for_call = module_id.clone();
    let result = ctx.module_call(|service| async move {
        service.synchronize_from_local(&module_id_for_call).await
    });

    match result {
        Ok(ModuleSyncOutcome::Packaged(package)) => {
            let package = *package;
            let install_result = package.install_result;
            let packaged_from = package.packaged_from;
            let packaged_source = packaged_from.display().to_string();
            let target_path = install_result.path.clone();
            writeln!(
                out,
                "{}",
                msg_modules::synchronize_flow::packaged_summary(
                    &install_result.manifest.id.to_string(),
                    &packaged_source,
                    &target_path
                )
            )?;
            let recorded_version = ModuleVersion(install_result.manifest.version.clone());
            ctx.record_audit(
                "module::synchronize",
                &module_id,
                Some(&recorded_version),
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("mode", "package")
                    .insert("packaged_from", packaged_from.display().to_string()),
            );
        }
        Ok(ModuleSyncOutcome::ExternalServices(dev_services)) => {
            writeln!(
                out,
                "{}",
                msg_modules::synchronize_flow::dev_services_header(
                    &dev_services.module_id.to_string()
                )
            )?;
            for svc in &dev_services.services {
                writeln!(
                    out,
                    "{}",
                    msg_modules::synchronize_flow::dev_service_entry(
                        &svc.service_id,
                        &svc.endpoint.to_string(),
                        svc.name.as_str(),
                        svc.description.as_deref()
                    )
                )?;
            }
            ctx.record_audit(
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
                    ctx.record_audit(
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
            render_service_error(out, msg_modules::synchronize_flow::ERROR_CONTEXT, &err)?;
            ctx.record_audit(
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

fn handle_release(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::release_flow::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };

    if ctx.output_sink().is_some() {
        let deps = ctx.deps_clone();
        let service = ctx.service();
        let fenrir_version = ctx.deps().config.app.version.clone();
        let module_id_for_task = module_id.clone();
        let future = async move {
            run_release_task(deps, service, module_id_for_task, fenrir_version).await
        };
        return Ok(CommandOutcome::AsyncTask(Box::pin(future)));
    }

    let module_id_for_call = module_id.clone();
    let fenrir_version = ctx.deps().config.app.version.clone();
    let fenrir_version_for_call = fenrir_version.clone();
    let result = ctx.module_call(|service| async move {
        service
            .release_override(&module_id_for_call, &fenrir_version_for_call)
            .await
    });

    match result {
        Ok(outcome) => {
            let install_result = outcome.install_result;
            writeln!(
                out,
                "{}",
                msg_modules::release_flow::success(
                    &install_result.manifest.id.to_string(),
                    &fenrir_version,
                    &install_result.manifest.version.to_string()
                )
            )?;
            let recorded_version = ModuleVersion(install_result.manifest.version.clone());
            ctx.record_audit(
                "module::release",
                &module_id,
                Some(&recorded_version),
                AuditOutcome::Success,
                AuditMetadata::default().insert("source", install_result.source.label()),
            );
            if outcome.dev_override_cleared {
                ctx.record_audit(
                    "module::sync::dev-agent-stop",
                    &module_id,
                    Some(&recorded_version),
                    AuditOutcome::Success,
                    AuditMetadata::default().insert("reason", "release"),
                );
            }
        }
        Err(err) => {
            render_service_error(out, msg_modules::release_flow::ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::release",
                &module_id,
                None,
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", module_error_code(&err))
                    .insert("error", err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_scaffold(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let parsed = match parse_scaffold_args(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            writeln!(out, "{err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };

    let (module_id, runtime) = parsed;
    let module_id_for_call = module_id.clone();
    let options = ModuleScaffoldOptions::new(runtime);
    let summary = match ctx.module_call(move |service| async move {
        service.scaffold_module(&module_id_for_call, options).await
    }) {
        Ok(summary) => summary,
        Err(err) => {
            render_service_error(out, msg_modules::scaffold_view::ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    render_scaffold_summary(out, &summary)?;
    Ok(CommandOutcome::Continue)
}

fn handle_release_dev_overrides(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if !args.is_empty() {
        writeln!(out, "{}", msg_modules::release_dev_overrides::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let fenrir_version = ctx.deps().config.app.version.clone();
    let fenrir_version_for_call = fenrir_version.clone();
    let result = ctx.module_call(|service| async move {
        service
            .release_all_dev_overrides(&fenrir_version_for_call)
            .await
    });

    match result {
        Ok(outcomes) => {
            if outcomes.is_empty() {
                writeln!(out, "{}", msg_modules::release_dev_overrides::EMPTY_STATE)?;
                return Ok(CommandOutcome::Continue);
            }
            writeln!(
                out,
                "{}",
                msg_modules::release_dev_overrides::releasing(outcomes.len())
            )?;
            for (module_id, outcome) in outcomes {
                let install_result = outcome.install_result;
                writeln!(
                    out,
                    "{}",
                    msg_modules::release_flow::success(
                        &install_result.manifest.id.to_string(),
                        &fenrir_version,
                        &install_result.manifest.version.to_string()
                    )
                )?;
                let recorded_version = ModuleVersion(install_result.manifest.version.clone());
                ctx.record_audit(
                    "module::release",
                    &module_id,
                    Some(&recorded_version),
                    AuditOutcome::Success,
                    AuditMetadata::default().insert("source", install_result.source.label()),
                );
                if outcome.dev_override_cleared {
                    ctx.record_audit(
                        "module::sync::dev-agent-stop",
                        &module_id,
                        Some(&recorded_version),
                        AuditOutcome::Success,
                        AuditMetadata::default().insert("reason", "release-all"),
                    );
                }
            }
        }
        Err(err) => {
            render_service_error(out, msg_modules::release_dev_overrides::ERROR_CONTEXT, &err)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_check_updates(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if !args.is_empty() {
        writeln!(out, "{}", msg_modules::check_flow::NO_ARGS_WARNING)?;
    }

    let fenrir_version = ctx.deps().config.app.version.clone();
    writeln!(out, "{}", msg_modules::check_flow::PROGRESS)?;

    let fenrir_version_for_call = fenrir_version.clone();
    let updates = match ctx.module_call(|service| async move {
        service.check_updates(Some(&fenrir_version_for_call)).await
    }) {
        Ok(updates) => updates,
        Err(err) => {
            render_service_error(out, msg_modules::check_flow::ERROR_CONTEXT, &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if updates.is_empty() {
        writeln!(out, "{}", msg_modules::check_flow::NO_MODULES)?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(
        msg_modules::check_flow::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for update in &updates {
        let status = if update.has_update {
            if update.compatible {
                msg_modules::check_flow::STATUS_AVAILABLE.to_string()
            } else {
                msg_modules::check_flow::STATUS_INCOMPATIBLE.to_string()
            }
        } else {
            msg_modules::check_flow::STATUS_CURRENT.to_string()
        };

        table.add_row(vec![
            update.module_id.to_string(),
            update.current_version.to_string(),
            update.latest_version.to_string(),
            status,
        ]);
    }

    table.render(out, "  ")?;

    let available_updates = updates
        .iter()
        .filter(|u| u.has_update && u.compatible)
        .count();
    if available_updates > 0 {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            msg_modules::check_flow::available_updates_hint(available_updates)
        )?;
    }

    Ok(CommandOutcome::Continue)
}

fn handle_logs(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.is_empty() {
        writeln!(out, "{}", msg_modules::logs_flow::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };

    let mut tail = None;
    let mut iter = args[1..].iter();
    while let Some(flag) = iter.next() {
        if *flag == "--tail" {
            let Some(value) = iter.next() else {
                writeln!(out, "{}", msg_modules::logs_flow::TAIL_REQUIRES_VALUE)?;
                return Ok(CommandOutcome::Continue);
            };
            tail = Some(match value.parse::<usize>() {
                Ok(n) => n,
                Err(_) => {
                    writeln!(out, "{}", msg_modules::logs_flow::invalid_tail(value))?;
                    return Ok(CommandOutcome::Continue);
                }
            });
        } else {
            writeln!(out, "{}", msg_modules::logs_flow::unknown_option(flag))?;
            return Ok(CommandOutcome::Continue);
        }
    }

    let module_id_for_call = module_id.clone();
    let result = ctx
        .runtime_call(move |service| async move { service.logs(&module_id_for_call, tail).await });

    match result {
        Ok(lines) => {
            writeln!(
                out,
                "{}",
                msg_modules::logs_flow::header(&module_id.to_string(), lines.len())
            )?;
            writeln!(out, "{}", msg_modules::logs_flow::SEPARATOR)?;
            for line in lines {
                writeln!(out, "{}", line)?;
            }
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::logs_flow::ERROR_CONTEXT, &err)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_env(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.is_empty() {
        writeln!(out, "{}", msg_modules::env_view::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }
    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };
    let module_id_for_call = module_id.clone();
    let env_result = ctx
        .runtime_call(move |service| async move { service.runtime_env(&module_id_for_call).await });
    match env_result {
        Ok(mut entries) => {
            entries.retain(|(key, _)| key.starts_with("FENRIR_"));
            if entries.is_empty() {
                writeln!(out, "{}", msg_modules::env_view::EMPTY_STATE)?;
                return Ok(CommandOutcome::Continue);
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            writeln!(out, "{}", msg_modules::env_view::TITLE)?;
            for (key, value) in entries {
                let rendered = if is_secret_env_key(&key) {
                    msg_modules::env_view::REDACTED.to_string()
                } else {
                    value
                };
                writeln!(out, "{key}={rendered}")?;
            }
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::env_view::ERROR_CONTEXT, &err)?;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn handle_stop_all(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if !args.is_empty() {
        writeln!(out, "{}", msg_modules::stop_all::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let virtual_id = ModuleId::new("all").expect("static module id");
    let result = ctx.runtime_call(|service| async move { service.stop_all_modules().await });
    match result {
        Ok(_) => {
            writeln!(out, "{}", msg_modules::stop_all::SUCCESS)?;
            ctx.record_audit(
                "module::stop-all",
                &virtual_id,
                None,
                AuditOutcome::Success,
                AuditMetadata::default(),
            );
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::stop_all::ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::stop-all",
                &virtual_id,
                None,
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", runtime_error_code(&err))
                    .insert("error", err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_services(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if args.len() > 1 {
        writeln!(out, "{}", msg_modules::services_view::USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let filter = match args.first() {
        Some(value) => match parse_module_id(out, value)? {
            Some(id) => Some(id.to_string()),
            None => return Ok(CommandOutcome::Continue),
        },
        None => None,
    };

    let registry = ctx.deps().services.registry();
    let mut rows: Vec<_> = registry
        .snapshot()
        .into_iter()
        .filter_map(ModuleServiceRow::from_snapshot)
        .collect();

    if let Some(target) = filter.as_ref() {
        rows.retain(|row| &row.module_id == target);
    }

    if rows.is_empty() {
        if let Some(target) = filter {
            writeln!(
                out,
                "{}",
                msg_modules::services_view::empty_for_module(&target)
            )?;
        } else {
            writeln!(out, "{}", msg_modules::services_view::EMPTY_STATE)?;
        }
        return Ok(CommandOutcome::Continue);
    }

    rows.sort_by(|a, b| {
        let left = (&a.module_id, a.service_sort_key());
        let right = (&b.module_id, b.service_sort_key());
        left.cmp(&right)
    });

    let runtime_infos =
        match ctx.runtime_call(|service| async move { service.list_running().await }) {
            Ok(list) => list
                .into_iter()
                .map(|info| (info.module_id.to_string(), info))
                .collect::<HashMap<_, _>>(),
            Err(err) => {
                render_runtime_error(
                    out,
                    msg_modules::services_view::RUNTIME_STATUS_ERROR_CONTEXT,
                    &err,
                )?;
                HashMap::new()
            }
        };

    render_module_service_table(out, &rows, &runtime_infos)?;
    Ok(CommandOutcome::Continue)
}

fn handle_start(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::lifecycle::START_USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };

    let module_id_for_call = module_id.clone();
    let result =
        ctx.runtime_call(|service| async move { service.start_module(&module_id_for_call).await });

    match result {
        Ok(info) => {
            let module_label = module_id.to_string();
            writeln!(
                out,
                "{}",
                msg_modules::lifecycle::start_success(&module_label, info.pid, info.port,)
            )?;
            ctx.record_audit(
                "module::start",
                &module_id,
                Some(&info.version),
                AuditOutcome::Success,
                runtime_success_metadata(&info),
            );
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::lifecycle::START_ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::start",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(runtime_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_stop(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::lifecycle::STOP_USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };

    let module_id_for_call = module_id.clone();
    let result = ctx.runtime_call(|service| async move { service.stop(&module_id_for_call).await });

    match result {
        Ok(()) => {
            writeln!(
                out,
                "{}",
                msg_modules::lifecycle::stop_success(&module_id.to_string())
            )?;
            ctx.record_audit(
                "module::stop",
                &module_id,
                None,
                AuditOutcome::Success,
                AuditMetadata::default().insert("status", "stopped"),
            );
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::lifecycle::STOP_ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::stop",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(runtime_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_restart(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() != 1 {
        writeln!(out, "{}", msg_modules::lifecycle::RESTART_USAGE)?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };

    let module_id_for_call = module_id.clone();
    let result =
        ctx.runtime_call(|service| async move { service.restart(&module_id_for_call).await });

    match result {
        Ok(info) => {
            let module_label = module_id.to_string();
            writeln!(
                out,
                "{}",
                msg_modules::lifecycle::restart_success(&module_label, info.pid, info.port,)
            )?;
            ctx.record_audit(
                "module::restart",
                &module_id,
                Some(&info.version),
                AuditOutcome::Success,
                runtime_success_metadata(&info),
            );
        }
        Err(err) => {
            render_runtime_error(out, msg_modules::lifecycle::RESTART_ERROR_CONTEXT, &err)?;
            ctx.record_audit(
                "module::restart",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(runtime_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_instances(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() != 1 {
        writeln!(out, "usage: modules instances <module-id>")?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };

    let module_id_for_call = module_id.clone();
    let result =
        ctx.runtime_call(
            |service| async move { service.runtime_instances(&module_id_for_call).await },
        );

    match result {
        Ok(instances) => render_module_instances(out, &module_id, &instances)?,
        Err(err) => {
            render_runtime_error(out, "Failed to list module instances.", &err)?;
            ctx.record_audit(
                "module::instances",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(runtime_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn handle_rolling_restart(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() != 1 {
        writeln!(out, "usage: modules rolling-restart <module-id>")?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };

    let module_id_for_call = module_id.clone();
    let result = ctx
        .runtime_call(|service| async move { service.rolling_restart(&module_id_for_call).await });

    match result {
        Ok(report) => {
            render_rolling_restart_report(out, &report)?;
            ctx.record_audit(
                "module::rolling_restart",
                &module_id,
                None,
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("health_verified", report.health_verified.to_string())
                    .insert(
                        "restarted_instances",
                        report.restarted_instances.len().to_string(),
                    ),
            );
        }
        Err(err) => {
            render_runtime_error(out, "Failed to rolling-restart module.", &err)?;
            ctx.record_audit(
                "module::rolling_restart",
                &module_id,
                None,
                AuditOutcome::Failure,
                module_error_metadata(runtime_error_code(&err), err.to_string()),
            );
        }
    }

    Ok(CommandOutcome::Continue)
}

fn render_module_instances(
    out: &mut dyn Write,
    module_id: &ModuleId,
    instances: &[ModuleRuntimeInstanceSnapshot],
) -> io::Result<()> {
    writeln!(out, "Instances for {module_id}:")?;
    if instances.is_empty() {
        writeln!(out, "  no runtime instances")?;
        return Ok(());
    }

    for instance in instances {
        writeln!(
            out,
            "  {}  status={} pid={} port={} endpoint={}",
            instance.instance_id,
            format_runtime_status(&instance.status),
            instance
                .pid
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            instance
                .port
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            instance.endpoint.as_deref().unwrap_or("-"),
        )?;
    }
    Ok(())
}

fn render_rolling_restart_report(
    out: &mut dyn Write,
    report: &ModuleRollingRestartReport,
) -> io::Result<()> {
    writeln!(out, "{}", report.note)?;
    writeln!(
        out,
        "health verified: {}",
        if report.health_verified { "yes" } else { "no" }
    )?;
    render_module_instances(out, &report.module_id, &report.restarted_instances)
}

fn render_canary_status(out: &mut dyn Write, status: &ModuleCanaryRoutingStatus) -> io::Result<()> {
    writeln!(out, "Canary routing for {}:", status.module_id)?;
    writeln!(
        out,
        "  strategy: {}",
        status
            .strategy
            .map(|value| format!("{value:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "unset".to_string())
    )?;
    writeln!(out, "  traffic percent: {}", status.traffic_percent)?;
    if status.configured_instances.is_empty() {
        writeln!(out, "  configured canary instances: none")?;
    } else {
        writeln!(
            out,
            "  configured canary instances: {}",
            status.configured_instances.join(", ")
        )?;
    }
    writeln!(out, "  active stable instances:")?;
    if status.active_stable_instances.is_empty() {
        writeln!(out, "    none")?;
    } else {
        for instance in &status.active_stable_instances {
            writeln!(
                out,
                "    {} ({})",
                instance.instance_id,
                instance.endpoint.as_deref().unwrap_or("-")
            )?;
        }
    }
    writeln!(out, "  active canary instances:")?;
    if status.active_canary_instances.is_empty() {
        writeln!(out, "    none")?;
    } else {
        for instance in &status.active_canary_instances {
            writeln!(
                out,
                "    {} ({})",
                instance.instance_id,
                instance.endpoint.as_deref().unwrap_or("-")
            )?;
        }
    }
    Ok(())
}

fn handle_canary(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    if args.len() < 2 {
        writeln!(
            out,
            "usage: modules canary <module-id> <status|start|set|clear> [percent] [instance-id ...]"
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(module_id) = parse_module_id(out, args[0])? else {
        return Ok(CommandOutcome::Continue);
    };
    match args[1] {
        "status" => {
            let module_id_for_call = module_id.clone();
            match ctx.runtime_call(|service| async move {
                service.canary_routing_status(&module_id_for_call).await
            }) {
                Ok(status) => render_canary_status(out, &status)?,
                Err(err) => render_runtime_error(out, "Failed to inspect canary routing.", &err)?,
            }
        }
        "start" => {
            let instance_ids = if args.len() > 2 {
                Some(args[2..].iter().map(|value| value.to_string()).collect())
            } else {
                None
            };
            let module_id_for_call = module_id.clone();
            match ctx.runtime_call(move |service| async move {
                service
                    .start_canary_routing(&module_id_for_call, instance_ids)
                    .await
            }) {
                Ok(status) => render_canary_status(out, &status)?,
                Err(err) => render_runtime_error(out, "Failed to start canary routing.", &err)?,
            }
        }
        "set" => {
            if args.len() < 3 {
                writeln!(
                    out,
                    "usage: modules canary <module-id> set <percent> [instance-id ...]"
                )?;
                return Ok(CommandOutcome::Continue);
            }
            let percent = match args[2].parse::<u8>() {
                Ok(value) => value,
                Err(_) => {
                    writeln!(out, "traffic percent must be an integer between 0 and 100")?;
                    return Ok(CommandOutcome::Continue);
                }
            };
            let instance_ids = if args.len() > 3 {
                Some(args[3..].iter().map(|value| value.to_string()).collect())
            } else {
                None
            };
            let module_id_for_call = module_id.clone();
            match ctx.runtime_call(move |service| async move {
                service
                    .set_canary_routing(&module_id_for_call, percent, instance_ids)
                    .await
            }) {
                Ok(status) => render_canary_status(out, &status)?,
                Err(err) => render_runtime_error(out, "Failed to update canary routing.", &err)?,
            }
        }
        "clear" => {
            let module_id_for_call = module_id.clone();
            match ctx.runtime_call(|service| async move {
                service.clear_canary_routing(&module_id_for_call).await
            }) {
                Ok(status) => render_canary_status(out, &status)?,
                Err(err) => render_runtime_error(out, "Failed to clear canary routing.", &err)?,
            }
        }
        _ => {
            writeln!(
                out,
                "usage: modules canary <module-id> <status|start|set|clear> [percent] [instance-id ...]"
            )?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn format_runtime_status(status: &crate::domain::module::ModuleRuntimeStatus) -> &'static str {
    match status {
        crate::domain::module::ModuleRuntimeStatus::Running => "running",
        crate::domain::module::ModuleRuntimeStatus::Stopped => "stopped",
        crate::domain::module::ModuleRuntimeStatus::Failed => "failed",
        crate::domain::module::ModuleRuntimeStatus::Starting => "starting",
        crate::domain::module::ModuleRuntimeStatus::Stopping => "stopping",
    }
}

fn handle_reload_overrides(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    let args = strip_module_keyword(args);
    let restart_running = !args.iter().any(|arg| {
        matches!(
            arg.to_ascii_lowercase().as_str(),
            "--no-restart" | "--no-restart-running"
        )
    });
    if args.iter().any(|arg| {
        arg.starts_with("--")
            && !matches!(
                arg.to_ascii_lowercase().as_str(),
                "--no-restart" | "--no-restart-running"
            )
    }) {
        writeln!(out, "usage: modules reload-overrides [--no-restart]")?;
        return Ok(CommandOutcome::Continue);
    }

    let loaded = match config::load() {
        Ok(cfg) => cfg,
        Err(err) => {
            writeln!(out, "failed to reload Fenrir config: {err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };
    let overrides = match crate::services::module::ModuleServiceOverrides::from_config(
        &loaded.modules.services,
        &loaded.modules.service_profiles,
    ) {
        Ok(overrides) => overrides,
        Err(err) => {
            writeln!(out, "invalid module override config: {err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };

    let reloaded = ctx.module_call(move |service| async move {
        service
            .reload_service_overrides(overrides, restart_running)
            .await
    });

    match reloaded {
        Ok(report) => {
            match report.status {
                crate::services::module::ModuleOverrideReloadStatus::Applied => {
                    if report.restarted_modules.is_empty() {
                        writeln!(
                            out,
                            "module overrides applied successfully; no running modules required restart"
                        )?;
                    } else {
                        let modules = report
                            .restarted_modules
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(
                            out,
                            "module overrides applied successfully; restarted: {modules}"
                        )?;
                    }
                }
                crate::services::module::ModuleOverrideReloadStatus::RolledBack => {
                    let restarted = report
                        .restarted_modules
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let rollback = report
                        .rollback_restarted_modules
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(
                        out,
                        "module overrides failed health checks and were rolled back; attempted restart: [{}], rollback restart: [{}]",
                        restarted,
                        rollback,
                    )?;
                }
                crate::services::module::ModuleOverrideReloadStatus::Failed => {
                    let modules = report
                        .restarted_modules
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(
                        out,
                        "module overrides applied with health check failures; restarted: [{modules}]"
                    )?;
                }
            }

            for module in report.modules.iter().filter(|entry| !entry.healthy) {
                writeln!(out, " - {}: {}", module.module_id, module.note)?;
            }
            if report.restart_running {
                let verified = report
                    .modules
                    .iter()
                    .filter(|entry| entry.health_checked && entry.healthy)
                    .count();
                writeln!(out, "health-verified modules: {verified}")?;
            }
        }
        Err(err) => {
            writeln!(out, "failed to reload module overrides: {err}")?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn parse_scaffold_args(args: &[&str]) -> Result<(ModuleId, ModuleScaffoldRuntime), String> {
    let mut module_id: Option<ModuleId> = None;
    let mut runtime = ModuleScaffoldRuntime::default();
    let mut idx = 0;

    while idx < args.len() {
        let current = args[idx];
        if current.eq_ignore_ascii_case("module") {
            idx += 1;
            continue;
        }
        if current.starts_with("--runtime") {
            let value = if let Some((_, tail)) = current.split_once('=') {
                tail
            } else {
                idx += 1;
                args.get(idx)
                    .copied()
                    .ok_or_else(|| msg_modules::scaffold_flow::MISSING_RUNTIME_VALUE.to_string())?
            };
            runtime = ModuleScaffoldRuntime::from_str(value).map_err(|_| {
                msg_modules::scaffold_flow::unknown_runtime(
                    value,
                    &ModuleScaffoldRuntime::variants().join(", "),
                )
            })?;
        } else if module_id.is_none() {
            module_id =
                Some(ModuleId::new(current).map_err(|err| {
                    msg_modules::parser::invalid_id_value(current, &err.to_string())
                })?);
        } else {
            return Err(msg_modules::scaffold_flow::extra_arguments(current));
        }

        idx += 1;
    }

    let module_id =
        module_id.ok_or_else(|| msg_modules::scaffold_flow::MISSING_MODULE.to_string())?;
    Ok((module_id, runtime))
}

fn parse_module_id(out: &mut dyn Write, raw: &str) -> io::Result<Option<ModuleId>> {
    match ModuleId::new(raw) {
        Ok(id) => Ok(Some(id)),
        Err(err) => {
            let message = err.to_string();
            writeln!(
                out,
                "{}",
                msg_modules::parser::invalid_module_id(raw, &message)
            )?;
            Ok(None)
        }
    }
}

fn strip_module_keyword<'a>(args: &'a [&'a str]) -> &'a [&'a str] {
    if let Some(first) = args.first() {
        if first.eq_ignore_ascii_case("module") {
            return &args[1..];
        }
    }
    args
}

fn is_secret_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if upper == "FENRIR_SERVICE_TOKEN" {
        return true;
    }
    let exempt_suffixes = ["_ISSUED_AT", "_EXPIRES_AT", "_TTL_SECS"];
    if upper.starts_with("FENRIR_SERVICE_TOKEN_")
        && exempt_suffixes.iter().any(|suffix| upper.ends_with(suffix))
    {
        return false;
    }
    if upper.contains("PASSWORD") || upper.contains("SECRET") {
        return true;
    }
    if upper.contains("PRIVATE_KEY") {
        return true;
    }
    if upper.ends_with("_KEY") {
        return true;
    }
    if upper.ends_with("_TOKEN") || upper.contains("_TOKEN_") {
        return true;
    }
    false
}

fn parse_module_target(
    target: &str,
    rest: &[&str],
) -> Result<(ModuleId, Option<ModuleVersion>), String> {
    let mut id_part = target;
    let mut version_part = None;

    if let Some((id, ver)) = target.split_once('@') {
        id_part = id;
        if !ver.is_empty() {
            version_part = Some(ver.to_string());
        }
    }

    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        if flag == &"--version" {
            let Some(value) = iter.next() else {
                return Err(msg_modules::parser::VERSION_REQUIRES_VALUE.to_string());
            };
            version_part = Some((*value).to_string());
        } else {
            return Err(msg_modules::parser::unknown_option(flag));
        }
    }

    let module_id = ModuleId::new(id_part).map_err(|err| {
        let message = err.to_string();
        msg_modules::parser::invalid_id_value(id_part, &message)
    })?;

    let version = match version_part {
        Some(ref value) => {
            let version_for_message = value.clone();
            Some(ModuleVersion::parse(value).map_err(|err| {
                let message = err.to_string();
                msg_modules::parser::invalid_version(&version_for_message, &message)
            })?)
        }
        None => None,
    };

    Ok((module_id, version))
}

fn render_module_service_table(
    out: &mut dyn Write,
    rows: &[ModuleServiceRow],
    runtime_infos: &HashMap<String, ModuleRuntimeInfo>,
) -> io::Result<()> {
    let mut table = Table::new(
        msg_modules::services_view::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for row in rows {
        let endpoint = if matches!(row.kind, ModuleServiceEntryKind::Runtime) {
            runtime_infos
                .get(&row.module_id)
                .and_then(|info| info.port.map(|port| format!("127.0.0.1:{port}")))
        } else {
            row.endpoint_hint().map(|hint| hint.to_string())
        }
        .unwrap_or_else(|| msg_modules::services_view::EMPTY_VALUE.to_string());

        let since = row
            .since
            .elapsed()
            .ok()
            .map(utils::format_brief_duration)
            .unwrap_or_else(|| msg_modules::services_view::EMPTY_VALUE.to_string());

        let note = row
            .note
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(msg_modules::services_view::EMPTY_VALUE);

        table.add_row(vec![
            row.module_id.clone(),
            row.service_label(),
            row.type_label().to_string(),
            row.status.label().to_string(),
            since,
            row.route(),
            endpoint,
            note.to_string(),
        ]);
    }

    table.render(out, "  ")
}

fn runtime_success_metadata(info: &ModuleRuntimeInfo) -> AuditMetadata {
    let mut metadata = AuditMetadata::default()
        .insert("status", format!("{:?}", info.status))
        .insert("kind", format!("{:?}", info.kind))
        .insert("restart_count", info.restart_count.to_string());
    if let Some(pid) = info.pid {
        metadata = metadata.insert("pid", pid.to_string());
    }
    if let Some(port) = info.port {
        metadata = metadata.insert("port", port.to_string());
    }
    metadata
}

#[derive(Clone)]
struct ModuleServiceRow {
    module_id: String,
    service_suffix: Option<String>,
    descriptor_id: String,
    status: ServiceStatus,
    since: SystemTime,
    note: Option<String>,
    endpoint_hint: Option<String>,
    kind: ModuleServiceEntryKind,
}

impl ModuleServiceRow {
    fn from_snapshot(snapshot: ServiceSnapshot) -> Option<Self> {
        let ServiceSnapshot {
            descriptor,
            status,
            since,
            note,
        } = snapshot;
        let (module_id, suffix) = parse_module_service_id(&descriptor.id)?;
        suffix.as_ref()?;
        let endpoint_hint = extract_endpoint_hint(note.as_deref());
        let kind = if is_dev_endpoint(note.as_deref()) {
            ModuleServiceEntryKind::DevOverride
        } else if is_declared_endpoint(note.as_deref()) {
            ModuleServiceEntryKind::Declared
        } else {
            ModuleServiceEntryKind::Runtime
        };
        Some(Self {
            module_id,
            service_suffix: suffix,
            descriptor_id: descriptor.id,
            status,
            since,
            note,
            endpoint_hint,
            kind,
        })
    }

    fn service_label(&self) -> String {
        match &self.service_suffix {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => msg_modules::services_view::RUNTIME_SERVICE_LABEL.to_string(),
        }
    }

    fn type_label(&self) -> &'static str {
        match self.kind {
            ModuleServiceEntryKind::Runtime => msg_modules::services_view::TYPE_RUNTIME,
            ModuleServiceEntryKind::DevOverride => msg_modules::services_view::TYPE_DEV,
            ModuleServiceEntryKind::Declared => msg_modules::services_view::TYPE_DECLARED,
        }
    }

    fn route(&self) -> String {
        format!("service://{}", self.descriptor_id)
    }

    fn service_sort_key(&self) -> &str {
        self.service_suffix
            .as_deref()
            .unwrap_or(msg_modules::services_view::RUNTIME_SERVICE_LABEL)
    }

    fn endpoint_hint(&self) -> Option<&str> {
        self.endpoint_hint.as_deref()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModuleServiceEntryKind {
    Runtime,
    DevOverride,
    Declared,
}

fn parse_module_service_id(id: &str) -> Option<(String, Option<String>)> {
    let stripped = id.strip_prefix("module:")?;
    if let Some((module, suffix)) = stripped.split_once("::") {
        Some((module.to_string(), Some(suffix.to_string())))
    } else {
        Some((stripped.to_string(), None))
    }
}

fn extract_endpoint_hint(note: Option<&str>) -> Option<String> {
    note.and_then(|value| {
        value
            .strip_prefix("dev endpoint ")
            .or_else(|| value.strip_prefix("endpoint "))
            .map(|rest| rest.to_string())
    })
}

fn is_dev_endpoint(note: Option<&str>) -> bool {
    note.map(|value| value.starts_with("dev endpoint "))
        .unwrap_or(false)
}

fn is_declared_endpoint(note: Option<&str>) -> bool {
    note.map(|value| value.starts_with("endpoint "))
        .unwrap_or(false)
}

const MODULE_SINGULAR_ALIASES: &[&str] = &["module"];
const MODULE_PLURAL_ALIASES: &[&str] = &["modules"];
const DISTRIBUTION_ALIASES: &[&str] = &["distribution", "distributions"];

#[derive(Clone, Copy)]
struct ModuleCommandSpec {
    action: &'static str,
    usage: &'static str,
    resource: ModuleResourceKind,
}

impl ModuleCommandSpec {
    const fn new(action: &'static str, usage: &'static str, resource: ModuleResourceKind) -> Self {
        Self {
            action,
            usage,
            resource,
        }
    }
}

#[derive(Clone, Copy)]
enum ModuleResourceKind {
    ModuleSingular,
    ModulePlural,
    Distribution,
}

impl ModuleResourceKind {
    fn matches(self, value: &str) -> bool {
        self.aliases()
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(value))
    }

    fn aliases(self) -> &'static [&'static str] {
        match self {
            ModuleResourceKind::ModuleSingular => MODULE_SINGULAR_ALIASES,
            ModuleResourceKind::ModulePlural => MODULE_PLURAL_ALIASES,
            ModuleResourceKind::Distribution => DISTRIBUTION_ALIASES,
        }
    }
}

const SEARCH_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "search",
    "search modules [pattern]",
    ModuleResourceKind::ModulePlural,
);
const INSTALL_DISTRIBUTION_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "install-distribution",
    "install distribution",
    ModuleResourceKind::Distribution,
);
const SYNCHRONIZE_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "synchronize",
    "synchronize module <name>",
    ModuleResourceKind::ModuleSingular,
);
const RELEASE_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "release",
    "release module <name>",
    ModuleResourceKind::ModuleSingular,
);
const SCAFFOLD_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "scaffold",
    "scaffold module <name> [--runtime rust|node|angular]",
    ModuleResourceKind::ModuleSingular,
);
const UNINSTALL_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "uninstall",
    "uninstall module <name>",
    ModuleResourceKind::ModuleSingular,
);
const CHECK_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "check-updates",
    "check modules",
    ModuleResourceKind::ModulePlural,
);
fn run_scoped_module_command(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
    spec: &ModuleCommandSpec,
) -> io::Result<CommandOutcome> {
    let Some(tail) = extract_resource_tail(args, out, spec) else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, spec.action, tail, out)
}

fn extract_resource_tail<'a>(
    args: &'a [&'a str],
    out: &mut dyn Write,
    spec: &ModuleCommandSpec,
) -> Option<&'a [&'a str]> {
    let Some((resource, tail)) = args.split_first() else {
        let _ = writeln!(out, "{}", msg_modules::scoped_command::usage(spec.usage));
        return None;
    };
    if spec.resource.matches(resource) {
        Some(tail)
    } else {
        let _ = writeln!(
            out,
            "{}",
            msg_modules::scoped_command::unknown_resource(resource)
        );
        let _ = writeln!(out, "{}", msg_modules::scoped_command::usage(spec.usage));
        None
    }
}

fn module_service_or_notify(
    deps: &CliDependencies,
    out: &mut dyn Write,
) -> io::Result<Option<Arc<ModuleService>>> {
    match deps.services.module_service() {
        Some(service) => Ok(Some(service)),
        None => {
            writeln!(out, "{}", msg_modules::service::UNAVAILABLE)?;
            Ok(None)
        }
    }
}
