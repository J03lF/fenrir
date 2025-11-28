use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;

use tracing::{info, warn};

use crate::audit::{AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ConfirmationRequest, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::module::{ModuleId, ModuleVersion};
use crate::services::module::{ModuleService, ModuleSyncOutcome};
use crate::utils;
use crate::utils::messages::cli::builtins::modules as msg_modules;

use super::ctx::{dev_services_metadata, module_error_metadata, ModulesCommandCtx};
use super::entries::{available_subcommands, resolve_module_subcommand};
use super::output::{
    module_error_code, render_distribution_plan, render_manifest, render_runtime_error,
    render_service_error,
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

pub(super) fn handle_logs_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    run_scoped_module_command(deps, args, out, &LOGS_SPEC)
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
        "uninstall" => handle_uninstall(ctx, out, args),
        "check-updates" => handle_check_updates(ctx, out, args),
        "logs" => handle_logs(ctx, out, args),
        other => {
            if matches!(other, "start" | "stop" | "restart") {
                writeln!(out, "{}", msg_modules::routing::lifecycle_disabled(other))?;
                writeln!(out, "{}", msg_modules::routing::LIFECYCLE_SUMMARY)?;
                return Ok(CommandOutcome::Continue);
            }
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

    let runtime_infos: HashMap<_, _> =
        match ctx.runtime_call(|service| async move { service.list_running().await }) {
            Ok(infos) => infos
                .into_iter()
                .map(|info| (info.module_id.to_string(), info))
                .collect(),
            Err(_) => HashMap::new(),
        };

    let mut table = Table::new(
        msg_modules::list_modules::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for module in modules {
        let module_id = module.manifest.id.clone();
        let source_label = module.source.label().to_string();
        if let Some(runtime_info) = runtime_infos.get(&module_id) {
            let duration = runtime_info
                .started_at
                .and_then(|started| started.elapsed().ok())
                .map(utils::format_brief_duration)
                .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string());

            table.add_row(vec![
                module_id,
                module.manifest.version.to_string(),
                source_label.clone(),
                msg_modules::list_modules::STATUS_RUNNING.to_string(),
                runtime_info
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string()),
                runtime_info
                    .port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| msg_modules::list_modules::EMPTY_VALUE.to_string()),
                duration,
            ]);
        } else {
            table.add_row(vec![
                module_id,
                module.manifest.version.to_string(),
                source_label,
                msg_modules::list_modules::STATUS_STOPPED.to_string(),
                msg_modules::list_modules::EMPTY_VALUE.to_string(),
                msg_modules::list_modules::EMPTY_VALUE.to_string(),
                msg_modules::list_modules::EMPTY_VALUE.to_string(),
            ]);
        }
    }

    table.render(out, "  ")?;
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

    render_distribution_plan(out, &plan)?;

    let actionable: Vec<_> = plan
        .iter()
        .filter(|entry| entry.action.requires_execution())
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
        Ok(install_result) => {
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
        if flag == &"--tail" {
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

const MODULE_ALIASES: &[&str] = &["module", "modules"];
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
    Module,
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
            ModuleResourceKind::Module => MODULE_ALIASES,
            ModuleResourceKind::Distribution => DISTRIBUTION_ALIASES,
        }
    }
}

const SEARCH_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "search",
    "search modules [pattern]",
    ModuleResourceKind::Module,
);
const INSTALL_DISTRIBUTION_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "install-distribution",
    "install distribution",
    ModuleResourceKind::Distribution,
);
const SYNCHRONIZE_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "synchronize",
    "synchronize module <name>",
    ModuleResourceKind::Module,
);
const RELEASE_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "release",
    "release module <name>",
    ModuleResourceKind::Module,
);
const UNINSTALL_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "uninstall",
    "uninstall module <name>",
    ModuleResourceKind::Module,
);
const CHECK_SPEC: ModuleCommandSpec =
    ModuleCommandSpec::new("check-updates", "check modules", ModuleResourceKind::Module);
const LOGS_SPEC: ModuleCommandSpec = ModuleCommandSpec::new(
    "logs",
    "logs module <name> [--tail N]",
    ModuleResourceKind::Module,
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
