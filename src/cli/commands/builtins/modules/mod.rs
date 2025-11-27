use std::collections::HashMap;
use std::future::Future;
use std::io::{self, Write};
use std::sync::Arc;

use tokio::runtime::{Handle, Runtime};
use tracing::{info, warn, Instrument};
use whoami;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CommandSubcommand, CompletionContext, CompletionKind, ConfirmationHandler,
    ConfirmationRequest, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::module::{
    ModuleId, ModuleInstallStatus, ModuleManifest, ModuleProgress, ModuleRegistryError,
    ModuleRuntimeError, ModuleServiceError, ModuleStartConfig, ModuleStorageError,
    ModuleVerificationError, ModuleVersion,
};
use crate::services::module::{
    DistributionAction, DistributionPlanEntry, ModuleService, ModuleSyncOutcome,
};
use crate::utils;

const DETAILS: &[&str] = &[
    "list modules                    – zeigt alle Module mit Runtime-Status",
    "search modules [pattern]        – durchsucht Registry",
    "show module <name[@version]>    – zeigt Manifest-Informationen",
    "install distribution            – installiert alle Module einer Distribution",
    "synchronize module <name>       – ersetzt Modul mit lokalen Änderungen",
    "release module <name>           – setzt Modul auf Distribution zurück",
    "uninstall module <name>         – entfernt ein installiertes Modul",
    "check modules                   – prüft verfügbare Updates",
    "logs module <name> [--tail N]   – zeigt Logs eines laufenden Moduls",
];

const MODULE_ALIASES: &[&str] = &["module"];

const MODULE_ID_ARGUMENT: CommandArgument = CommandArgument {
    name: "module",
    optional: false,
    variadic: false,
    completion: CompletionKind::Dynamic(complete_module_ids),
};

const MODULE_LOG_TAIL_ARGUMENT: CommandArgument = CommandArgument {
    name: "--tail",
    optional: true,
    variadic: false,
    completion: CompletionKind::Static(&["--tail"]),
};

const MODULE_RESOURCE_OPTIONS: &[&str] = &["module", "modules"];
const MODULE_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(MODULE_RESOURCE_OPTIONS));
const MODULE_PATTERN_ARGUMENT: CommandArgument = CommandArgument::optional("pattern");

const SEARCH_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_ARGUMENT, MODULE_PATTERN_ARGUMENT];
const SEARCH_SHAPE: CommandShape = CommandShape::new("search", &[], SEARCH_ARGUMENTS, &[]);

const DISTRIBUTION_RESOURCE_OPTIONS: &[&str] = &["distribution", "distributions"];
const DISTRIBUTION_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(DISTRIBUTION_RESOURCE_OPTIONS));

const INSTALL_ARGUMENTS: &[CommandArgument] = &[DISTRIBUTION_RESOURCE_ARGUMENT];
const INSTALL_SHAPE: CommandShape =
    CommandShape::new("install", &["import"], INSTALL_ARGUMENTS, &[]);

const UNINSTALL_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_ARGUMENT, MODULE_ID_ARGUMENT];
const UNINSTALL_SHAPE: CommandShape =
    CommandShape::new("uninstall", &["remove"], UNINSTALL_ARGUMENTS, &[]);

const CHECK_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_ARGUMENT];
const CHECK_SHAPE: CommandShape = CommandShape::new("check", &[], CHECK_ARGUMENTS, &[]);

const SYNCHRONIZE_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_ARGUMENT, MODULE_ID_ARGUMENT];
const SYNCHRONIZE_SHAPE: CommandShape =
    CommandShape::new("synchronize", &["sync"], SYNCHRONIZE_ARGUMENTS, &[]);

const RELEASE_ARGUMENTS: &[CommandArgument] = &[MODULE_RESOURCE_ARGUMENT, MODULE_ID_ARGUMENT];
const RELEASE_SHAPE: CommandShape = CommandShape::new("release", &[], RELEASE_ARGUMENTS, &[]);

const LOGS_ARGUMENTS: &[CommandArgument] = &[
    MODULE_RESOURCE_ARGUMENT,
    MODULE_ID_ARGUMENT,
    MODULE_LOG_TAIL_ARGUMENT,
];
const LOGS_SHAPE: CommandShape = CommandShape::new("logs", &["tail"], LOGS_ARGUMENTS, &[]);

const SEARCH_DETAILS: &[&str] = &["search modules [pattern] – durchsucht die Modul-Registry"];
const INSTALL_DETAILS: &[&str] =
    &["install distribution – installiert/aktualisiert alle kompatiblen Module"];
const SYNCHRONIZE_DETAILS: &[&str] = &[
    "synchronize module <name> – ersetzt das Modul durch lokale Dateien",
    "Nutze [modules.dev_sources] base_path, um Dev-Builds automatisch zu verwenden",
    ".fenrir-dev.toml kann Dev-Service-Endpunkte definieren; sync module registriert dann diese Services statt Artefakte zu packen",
];
const RELEASE_DETAILS: &[&str] =
    &["release module <name> – setzt lokale Anpassungen zurück zur Distribution"];
const UNINSTALL_DETAILS: &[&str] = &["uninstall module <name> – entfernt ein Modul"];
const CHECK_DETAILS: &[&str] = &["check modules – prüft verfügbare Modul-Updates"];
const LOGS_DETAILS: &[&str] = &["logs module <name> [--tail N] – zeigt Laufzeit-Logs"];

pub fn search_command() -> CommandEntry {
    CommandEntry::with_shape(
        "search",
        "Durchsucht signierte Module in der Registry",
        "search modules [pattern]",
        SEARCH_DETAILS,
        handle_search_command,
        SEARCH_SHAPE,
    )
}

pub fn install_command() -> CommandEntry {
    CommandEntry::with_shape(
        "install",
        "Installiert Module einer Fenrir-Distribution",
        "install distribution",
        INSTALL_DETAILS,
        handle_install_command,
        INSTALL_SHAPE,
    )
}

pub fn synchronize_command() -> CommandEntry {
    CommandEntry::with_shape(
        "synchronize",
        "Übernimmt lokale Moduländerungen",
        "synchronize module <name>",
        SYNCHRONIZE_DETAILS,
        handle_synchronize_command,
        SYNCHRONIZE_SHAPE,
    )
}

pub fn release_command() -> CommandEntry {
    CommandEntry::with_shape(
        "release",
        "Setzt lokale Moduländerungen zurück",
        "release module <name>",
        RELEASE_DETAILS,
        handle_release_command,
        RELEASE_SHAPE,
    )
}

pub fn uninstall_command() -> CommandEntry {
    CommandEntry::with_shape(
        "uninstall",
        "Entfernt installierte Module",
        "uninstall module <name>",
        UNINSTALL_DETAILS,
        handle_uninstall_command,
        UNINSTALL_SHAPE,
    )
}

pub fn check_command() -> CommandEntry {
    CommandEntry::with_shape(
        "check",
        "Prüft verfügbare Modul-Updates",
        "check modules",
        CHECK_DETAILS,
        handle_check_command,
        CHECK_SHAPE,
    )
}

pub fn logs_command() -> CommandEntry {
    CommandEntry::with_shape(
        "logs",
        "Zeigt Laufzeit-Logs eines Moduls",
        "logs module <name> [--tail N]",
        LOGS_DETAILS,
        handle_logs_command,
        LOGS_SHAPE,
    )
}

const MODULE_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("list", &[], &[], "Installierte Module auflisten"),
    CommandSubcommand::new("search", &[], &[], "Registry nach Modulen durchsuchen"),
    CommandSubcommand::new(
        "info",
        &[],
        &[MODULE_ID_ARGUMENT],
        "Manifest eines Moduls anzeigen",
    ),
    CommandSubcommand::new(
        "install-distribution",
        &["install_distribution"],
        &[],
        "Alle Module für eine Fenrir-Distribution installieren",
    ),
    CommandSubcommand::new(
        "synchronize",
        &["sync"],
        &[MODULE_ID_ARGUMENT],
        "Lokale Änderungen eines Moduls übernehmen",
    ),
    CommandSubcommand::new(
        "release",
        &[],
        &[MODULE_ID_ARGUMENT],
        "Modul auf Distribution zurücksetzen",
    ),
    CommandSubcommand::new(
        "uninstall",
        &["remove"],
        &[MODULE_ID_ARGUMENT],
        "Modul deinstallieren",
    ),
    CommandSubcommand::new(
        "check-updates",
        &["check_updates"],
        &[],
        "Verfügbare Modul-Updates prüfen",
    ),
    CommandSubcommand::new(
        "logs",
        &[],
        &[MODULE_ID_ARGUMENT, MODULE_LOG_TAIL_ARGUMENT],
        "Modullogs anzeigen",
    ),
];

const MODULES_SHAPE: CommandShape =
    CommandShape::new("modules", MODULE_ALIASES, &[], MODULE_SUBCOMMANDS);

pub(crate) fn resolve_module_subcommand(alias: &str) -> Option<&'static str> {
    MODULE_SUBCOMMANDS
        .iter()
        .find(|entry| {
            entry.name == alias || entry.aliases.iter().any(|candidate| *candidate == alias)
        })
        .map(|entry| entry.name)
}

pub(crate) fn complete_module_ids(
    deps: &CliDependencies,
    _ctx: &CompletionContext<'_>,
) -> Vec<String> {
    let Some(service) = deps.services.module_service() else {
        return Vec::new();
    };

    match run_module_call(Arc::clone(&service), |svc| async move {
        svc.list_installed().await
    }) {
        Ok(installed) => {
            let mut ids: Vec<_> = installed
                .into_iter()
                .map(|module| module.manifest.id)
                .collect();
            ids.sort();
            ids
        }
        Err(err) => {
            warn!(target = "cli::completion", error = %err, "module id completion failed");
            Vec::new()
        }
    }
}

// Helper to run async code from sync command handler
fn run_module_future<F, Fut, T>(factory: F) -> Result<T, ModuleServiceError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                        "runtime init failed: {err}"
                    )))
                })?
                .block_on(factory().in_current_span())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleServiceError::Storage(
                ModuleStorageError::Unavailable(format!("blocking thread panicked: {err:?}")),
            ))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                "runtime init failed: {err}"
            )))
        })?
        .block_on(factory().in_current_span())
}

// Helper to run runtime futures (similar to run_module_future)
fn run_runtime_future<F, Fut, T>(factory: F) -> Result<T, ModuleRuntimeError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}"))
                })?
                .block_on(factory().in_current_span())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleRuntimeError::InvalidState(format!(
                "blocking thread panicked: {err:?}"
            )))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}")))?
        .block_on(factory().in_current_span())
}

fn run_module_call<F, Fut, T>(
    service: Arc<ModuleService>,
    factory: F,
) -> Result<T, ModuleServiceError>
where
    F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
    T: Send + 'static,
{
    run_module_future(move || {
        let service = Arc::clone(&service);
        factory(service)
    })
}

fn run_module_runtime_call<F, Fut, T>(
    service: Arc<ModuleService>,
    factory: F,
) -> Result<T, ModuleRuntimeError>
where
    F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
    T: Send + 'static,
{
    let span = tracing::Span::current();
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let _enter = span.enter();
            Runtime::new()
                .map_err(|err| {
                    ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}"))
                })?
                .block_on(async move {
                    let service = Arc::clone(&service);
                    factory(service).await
                }.in_current_span())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleRuntimeError::InvalidState(format!(
                "blocking thread panicked: {err:?}"
            )))
        });
    }
    let _enter = span.enter();
    Runtime::new()
        .map_err(|err| ModuleRuntimeError::InvalidState(format!("runtime init failed: {err}")))?
        .block_on(async move {
            let service = Arc::clone(&service);
            factory(service).await
        }.in_current_span())
}

fn module_tail<'a>(args: &'a [&'a str], out: &mut dyn Write, usage: &str) -> Option<&'a [&'a str]> {
    let Some((resource, tail)) = args.split_first() else {
        let _ = writeln!(out, "Nutzung: {usage}");
        return None;
    };

    if resource.eq_ignore_ascii_case("module") || resource.eq_ignore_ascii_case("modules") {
        Some(tail)
    } else {
        let _ = writeln!(out, "Unbekannte Ressource: {resource}");
        let _ = writeln!(out, "Nutzung: {usage}");
        None
    }
}

pub(crate) fn run_module_command(
    deps: &CliDependencies,
    action: &str,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some(service) = deps.services.module_service() else {
        writeln!(
            out,
            "Modul-Service nicht verfügbar – bitte Boot-Logs prüfen."
        )?;
        return Ok(CommandOutcome::Continue);
    };
    let ctx = ModulesCommandCtx::new(deps, Arc::clone(&service));
    dispatch_module(action, &ctx, out, args)
}

fn handle_search_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "search modules [pattern]") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "search", tail, out)
}

fn handle_install_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: install distribution")?;
        return Ok(CommandOutcome::Continue);
    };

    if resource.eq_ignore_ascii_case("distribution")
        || resource.eq_ignore_ascii_case("distributions")
    {
        return run_module_command(deps, "install-distribution", tail, out);
    }

    writeln!(out, "Unbekannte Ressource: {resource}")?;
    writeln!(out, "Nutzung: install distribution")?;
    Ok(CommandOutcome::Continue)
}

fn handle_synchronize_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "synchronize module <name>") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "synchronize", tail, out)
}

fn handle_release_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "release module <name>") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "release", tail, out)
}

fn handle_uninstall_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "uninstall module <name>") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "uninstall", tail, out)
}

fn handle_check_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "check modules") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "check-updates", tail, out)
}

fn handle_logs_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some(tail) = module_tail(args, out, "logs module <name> [--tail N]") else {
        return Ok(CommandOutcome::Continue);
    };
    run_module_command(deps, "logs", tail, out)
}

struct ModulesCommandCtx<'a> {
    deps: &'a CliDependencies,
    service: Arc<ModuleService>,
}

impl<'a> ModulesCommandCtx<'a> {
    fn new(deps: &'a CliDependencies, service: Arc<ModuleService>) -> Self {
        Self { deps, service }
    }

    fn service(&self) -> Arc<ModuleService> {
        Arc::clone(&self.service)
    }

    fn deps_clone(&self) -> CliDependencies {
        self.deps.clone()
    }

    fn output_sink(&self) -> Option<Arc<dyn CommandOutput>> {
        self.deps.output()
    }

    fn module_call<F, Fut, T>(&self, factory: F) -> Result<T, ModuleServiceError>
    where
        F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
        T: Send + 'static,
    {
        let service = self.service();
        run_module_future(move || factory(service))
    }

    fn runtime_call<F, Fut, T>(&self, factory: F) -> Result<T, ModuleRuntimeError>
    where
        F: FnOnce(Arc<ModuleService>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, ModuleRuntimeError>> + Send + 'static,
        T: Send + 'static,
    {
        let service = self.service();
        run_runtime_future(move || factory(service))
    }

    fn record_audit(
        &self,
        action: &str,
        module: &ModuleId,
        version: Option<&ModuleVersion>,
        outcome: AuditOutcome,
        metadata: AuditMetadata,
    ) {
        let mut metadata = metadata.insert("transport", "cli").insert(
            "command",
            format!("modules {}", action.split("::").last().unwrap_or(action)),
        );

        if let Some(version) = version {
            metadata = metadata.insert("version", version.to_string());
        }

        let target = module.as_str().to_string();
        let event = AuditEvent::builder()
            .actor(module_cli_actor(self.deps.session_actor()))
            .action(action)
            .target(target)
            .outcome(outcome)
            .metadata(metadata)
            .build();

        match event {
            Ok(event) => {
                if let Err(err) = self.deps.services.record_audit(event) {
                    warn!(error = %err, "module audit append failed");
                }
            }
            Err(err) => warn!(error = %err, "module audit build failed"),
        }
    }
}

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "modules",
        "Modulverwaltung",
        "modules <subcommand>",
        DETAILS,
        handle,
        MODULES_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    info!(command = "modules", "modules command invoked");
    let Some(service) = deps.services.module_service() else {
        writeln!(
            out,
            "Modul-Service nicht verfügbar – bitte Boot-Logs prüfen."
        )?;
        return Ok(CommandOutcome::Continue);
    };

    let ctx = ModulesCommandCtx::new(deps, Arc::clone(&service));
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
            "Unbekannter Subcommand '{subcommand_name}'. Nutze 'help modules'."
        )?;
        writeln!(out, "Verfügbare Subcommands: {}", available_subcommands())?;
    }

    Ok(CommandOutcome::Continue)
}

fn available_subcommands() -> String {
    MODULE_SUBCOMMANDS
        .iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>()
        .join(", ")
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
                writeln!(
                    out,
                    "Modul-Lifecycle ist automatisiert – '{}' module ist nicht mehr verfügbar.",
                    other
                )?;
                writeln!(
                    out,
                    "Fenrir startet/stoppt Module beim Boot und während Distribution-Imports automatisch."
                )?;
                return Ok(CommandOutcome::Continue);
            }
            warn!(
                target = "cli::modules",
                subcommand = other,
                "missing module handler mapping"
            );
            writeln!(out, "Subcommand '{other}' ist derzeit nicht implementiert.")?;
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
        writeln!(out, "'list modules' erwartet keine weiteren Argumente.")?;
    }

    let modules = match ctx.module_call(|service| async move { service.list_installed().await }) {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(
                out,
                "Installierte Module konnten nicht geladen werden",
                &err,
            )?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if modules.is_empty() {
        writeln!(out, "Installierte Module")?;
        writeln!(out, "====================")?;
        writeln!(out, "(keine Module installiert)")?;
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

    let mut table = Table::new(vec![
        "Modul".to_string(),
        "Version".to_string(),
        "Quelle".to_string(),
        "Status".to_string(),
        "PID".to_string(),
        "Port".to_string(),
        "Laufzeit".to_string(),
    ]);

    for module in modules {
        let module_id = module.manifest.id.clone();
        let source_label = module.source.label().to_string();
        if let Some(runtime_info) = runtime_infos.get(&module_id) {
            let duration = runtime_info
                .started_at
                .and_then(|started| started.elapsed().ok())
                .map(utils::format_brief_duration)
                .unwrap_or_else(|| "-".to_string());

            table.add_row(vec![
                module_id,
                module.manifest.version.to_string(),
                source_label.clone(),
                "Running".to_string(),
                runtime_info
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                runtime_info
                    .port
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                duration,
            ]);
        } else {
            table.add_row(vec![
                module_id,
                module.manifest.version.to_string(),
                source_label,
                "Stopped".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
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

    let pattern = args.get(0).map(|value| (*value).to_string());
    let pattern_for_query = pattern.clone();

    let modules = match ctx.module_call(|service| async move {
        let query = ModuleSearchQuery::new(pattern_for_query);
        service.search(query).await
    }) {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(out, "Registry-Suche fehlgeschlagen", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    writeln!(out, "Registry-Ergebnisse")?;
    writeln!(out, "==================")?;

    if modules.is_empty() {
        writeln!(out, "Keine Treffer.")?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(vec![
        "Modul".to_string(),
        "Version".to_string(),
        "Beschreibung".to_string(),
    ]);

    for entry in modules {
        table.add_row(vec![
            entry.id.to_string(),
            entry.version.to_string(),
            entry.description.unwrap_or_else(|| "-".to_string()),
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
        writeln!(out, "Use: show module <name[@version]>")?;
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
            render_service_error(out, "Manifest konnte nicht geladen werden", &err)?;
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
            "Fenrir-Version wird automatisch verwendet ({}).",
            ctx.deps.config.app.version
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let fenrir_version = ctx.deps.config.app.version.clone();

    let version_for_call = fenrir_version.clone();
    let plan = match ctx
        .module_call(|service| async move { service.distribution_plan(&version_for_call).await })
    {
        Ok(plan) => plan,
        Err(err) => {
            render_service_error(out, "Distribution konnte nicht ermittelt werden", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if plan.is_empty() {
        writeln!(
            out,
            "Keine Module für Fenrir {} in der Registry gefunden.",
            fenrir_version
        )?;
        return Ok(CommandOutcome::Continue);
    }

    render_distribution_plan(out, &plan)?;

    let actionable: Vec<_> = plan
        .iter()
        .cloned()
        .filter(|entry| entry.action.requires_execution())
        .collect();

    if actionable.is_empty() {
        writeln!(
            out,
            "Alle Module für Fenrir {} sind bereits installiert.",
            fenrir_version
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let confirmation = ConfirmationRequest::new(
        &format!("? Apply distribution plan? [Y/n] "),
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
        writeln!(out, "Use: uninstall module <name>")?;
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
            writeln!(out, "\n[REMOVE] {}", module_id)?;
            writeln!(out, "         ├── Stopping service hooks...     DONE")?;
            writeln!(out, "         ├── Unlinking module files...     DONE")?;
            writeln!(out, "         └── Cleaning up configurations... DONE")?;
            writeln!(out)?;
            writeln!(out, "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' removed successfully.", module_id)?;
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
            render_service_error(out, "Deinstallation fehlgeschlagen", &err)?;
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
        writeln!(out, "Use: synchronize module <name>")?;
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
            let install_result = package.install_result;
            let packaged_from = package.packaged_from;
            writeln!(
                out,
                "✓ Modul {} wurde mit lokalen Dateien synchronisiert (Quelle: {}, installiert nach {}).",
                install_result.manifest.id,
                packaged_from.display(),
                install_result.path
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
                "✓ Modul {} wurde auf Dev-Service-Endpunkte umgestellt:",
                dev_services.module_id
            )?;
            for svc in &dev_services.services {
                if let Some(description) = svc.description.as_deref() {
                    writeln!(
                        out,
                        "  - {} @ {} ({}) – {}",
                        svc.service_id, svc.endpoint, svc.name, description
                    )?;
                } else {
                    writeln!(
                        out,
                        "  - {} @ {} ({})",
                        svc.service_id, svc.endpoint, svc.name
                    )?;
                }
            }
            ctx.record_audit(
                "module::synchronize",
                &module_id,
                Some(&dev_services.version),
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("mode", "dev_services")
                    .insert(
                        "service_ids",
                        dev_services
                            .services
                            .iter()
                            .map(|svc| svc.service_id.clone())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
            );
        }
        Err(err) => {
            render_service_error(out, "Synchronisation fehlgeschlagen", &err)?;
            ctx.record_audit(
                "module::synchronize",
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

fn handle_release(
    ctx: &ModulesCommandCtx,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<CommandOutcome> {
    if args.len() != 1 {
        writeln!(out, "Use: release module <name>")?;
        return Ok(CommandOutcome::Continue);
    }

    let module_id = match parse_module_id(out, args[0])? {
        Some(id) => id,
        None => return Ok(CommandOutcome::Continue),
    };

    if ctx.output_sink().is_some() {
        let deps = ctx.deps_clone();
        let service = ctx.service();
        let fenrir_version = ctx.deps.config.app.version.clone();
        let module_id_for_task = module_id.clone();
        let future = async move {
            run_release_task(deps, service, module_id_for_task, fenrir_version).await
        };
        return Ok(CommandOutcome::AsyncTask(Box::pin(future)));
    }

    let module_id_for_call = module_id.clone();
    let fenrir_version = ctx.deps.config.app.version.clone();
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
                "✓ Modul {} läuft wieder mit der Distribution (Fenrir {}) v{}.",
                install_result.manifest.id, fenrir_version, install_result.manifest.version
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
            render_service_error(out, "Release fehlgeschlagen", &err)?;
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
        writeln!(out, "'check modules' erwartet keine Argumente.")?;
    }

    let fenrir_version = ctx.deps.config.app.version.clone();
    writeln!(out, "Prüfe Updates...")?;

    let fenrir_version_for_call = fenrir_version.clone();
    let updates = match ctx.module_call(|service| async move {
        service.check_updates(Some(&fenrir_version_for_call)).await
    }) {
        Ok(updates) => updates,
        Err(err) => {
            render_service_error(out, "Update-Prüfung fehlgeschlagen", &err)?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if updates.is_empty() {
        writeln!(out, "Keine installierten Module.")?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(vec![
        "Modul".to_string(),
        "Installiert".to_string(),
        "Verfügbar".to_string(),
        "Status".to_string(),
    ]);

    for update in &updates {
        let status = if update.has_update {
            if update.compatible {
                "Update verfügbar".to_string()
            } else {
                "Inkompatibel".to_string()
            }
        } else {
            "Aktuell".to_string()
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
            "💡 {} Update(s) verfügbar. Nutze 'install distribution' zum Aktualisieren.",
            available_updates
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
        writeln!(out, "Use: logs module <name> [--tail N]")?;
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
                writeln!(out, "--tail benötigt einen Wert")?;
                return Ok(CommandOutcome::Continue);
            };
            tail = Some(match value.parse::<usize>() {
                Ok(n) => n,
                Err(_) => {
                    writeln!(out, "Ungültiger tail-Wert: {}", value)?;
                    return Ok(CommandOutcome::Continue);
                }
            });
        } else {
            writeln!(out, "Unbekannte Option {}", flag)?;
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
                "Logs für Modul {} (Zeilen: {})",
                module_id,
                lines.len()
            )?;
            writeln!(out, "========================================")?;
            for line in lines {
                writeln!(out, "{}", line)?;
            }
        }
        Err(err) => {
            render_runtime_error(out, "Log-Abfrage fehlgeschlagen", &err)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn parse_module_id(out: &mut dyn Write, raw: &str) -> io::Result<Option<ModuleId>> {
    match ModuleId::new(raw) {
        Ok(id) => Ok(Some(id)),
        Err(err) => {
            writeln!(out, "Ungültige Modul-ID '{}': {}", raw, err)?;
            Ok(None)
        }
    }
}

fn render_manifest(out: &mut dyn Write, manifest: &ModuleManifest) -> io::Result<()> {
    writeln!(out, "Manifest für Modul {}", manifest.id)?;
    writeln!(out, "===============================")?;
    writeln!(out, "Version: {}", manifest.version)?;

    if let Some(title) = &manifest.title {
        writeln!(out, "Titel: {title}")?;
    }
    if let Some(desc) = &manifest.description {
        writeln!(out, "Beschreibung: {desc}")?;
    }
    if let Some(license) = &manifest.license {
        writeln!(out, "Lizenz: {license}")?;
    }
    if !manifest.authors.is_empty() {
        writeln!(out, "Autoren: {}", manifest.authors.join(", "))?;
    }
    if let Some(req) = &manifest.fenrir_version {
        writeln!(out, "Kompatibel mit Fenrir: {req}")?;
    }
    if !manifest.tags.is_empty() {
        writeln!(out, "Tags: {}", manifest.tags.join(","))?;
    }

    writeln!(out, "Download: {}", manifest.artifact.download_url)?;
    writeln!(out, "Checksumme: {}", manifest.artifact.checksum.hash)?;
    writeln!(out, "Signatur-Schlüssel: {}", manifest.signature.key_id)?;

    Ok(())
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
                return Err("--version benötigt einen Wert".to_string());
            };
            version_part = Some((*value).to_string());
        } else {
            return Err(format!("Unbekannte Option {flag}"));
        }
    }

    let module_id =
        ModuleId::new(id_part).map_err(|err| format!("Ungültige Modul-ID `{id_part}`: {err}"))?;

    let version = match version_part {
        Some(ref value) => Some(
            ModuleVersion::parse(value)
                .map_err(|err| format!("Ungültige Version `{value}`: {err}"))?,
        ),
        None => None,
    };

    Ok((module_id, version))
}

fn render_service_error(
    out: &mut dyn Write,
    context: &str,
    err: &ModuleServiceError,
) -> io::Result<()> {
    writeln!(
        out,
        "✗ {}: {} ({})",
        context,
        module_error_message(err),
        module_error_code(err)
    )
}

fn module_error_code(err: &ModuleServiceError) -> &'static str {
    match err {
        ModuleServiceError::Registry(_) => "registry_error",
        ModuleServiceError::Storage(_) => "storage_error",
        ModuleServiceError::Verification(_) => "verification_error",
    }
}

fn module_error_message(err: &ModuleServiceError) -> String {
    match err {
        ModuleServiceError::Registry(inner) => match inner {
            ModuleRegistryError::Unavailable(msg) => format!("Registry nicht verfügbar: {}", msg),
            ModuleRegistryError::NotFound { module } => {
                format!("Modul '{}' wurde nicht gefunden", module)
            }
            ModuleRegistryError::Protocol(msg) => format!("Protokollfehler: {}", msg),
        },
        ModuleServiceError::Storage(inner) => match inner {
            ModuleStorageError::Unavailable(msg) | ModuleStorageError::Io(msg) => msg.clone(),
            ModuleStorageError::InvalidState(msg) => format!("Ungültiger Zustand: {}", msg),
        },
        ModuleServiceError::Verification(inner) => match inner {
            ModuleVerificationError::Signature(msg) | ModuleVerificationError::Checksum(msg) => {
                msg.clone()
            }
            ModuleVerificationError::Unsupported => {
                "Signaturalgorithmus wird nicht unterstützt".to_string()
            }
        },
    }
}

fn render_runtime_error(
    out: &mut dyn Write,
    context: &str,
    err: &ModuleRuntimeError,
) -> io::Result<()> {
    writeln!(
        out,
        "✗ {}: {} ({})",
        context,
        runtime_error_message(err),
        runtime_error_code(err)
    )
}

fn runtime_error_code(err: &ModuleRuntimeError) -> &'static str {
    match err {
        ModuleRuntimeError::NotInstalled { .. } => "not_installed",
        ModuleRuntimeError::AlreadyRunning { .. } => "already_running",
        ModuleRuntimeError::NotRunning { .. } => "not_running",
        ModuleRuntimeError::StartFailed { .. } => "start_failed",
        ModuleRuntimeError::StopFailed { .. } => "stop_failed",
        ModuleRuntimeError::PortInUse { .. } => "port_in_use",
        ModuleRuntimeError::InvalidState(_) => "invalid_state",
        ModuleRuntimeError::Io(_) => "io_error",
    }
}

fn runtime_error_message(err: &ModuleRuntimeError) -> String {
    match err {
        ModuleRuntimeError::NotInstalled { module_id } => {
            format!("Modul '{}' ist nicht installiert", module_id)
        }
        ModuleRuntimeError::AlreadyRunning { module_id } => {
            format!("Modul '{}' läuft bereits", module_id)
        }
        ModuleRuntimeError::NotRunning { module_id } => {
            format!("Modul '{}' läuft nicht", module_id)
        }
        ModuleRuntimeError::StartFailed { module_id, reason } => {
            format!("Start von '{}' fehlgeschlagen: {}", module_id, reason)
        }
        ModuleRuntimeError::StopFailed { module_id, reason } => {
            format!("Stop von '{}' fehlgeschlagen: {}", module_id, reason)
        }
        ModuleRuntimeError::PortInUse { port } => {
            format!("Port {} ist bereits belegt", port)
        }
        ModuleRuntimeError::InvalidState(msg) => format!("Ungültiger Zustand: {}", msg),
        ModuleRuntimeError::Io(msg) => format!("I/O-Fehler: {}", msg),
    }
}

fn render_progress(
    out: &mut dyn Write,
    index: usize,
    total: usize,
    stage_fraction: f64,
    message: &str,
) -> io::Result<()> {
    #![allow(dead_code)]
    let total_f = total.max(1) as f64;
    let progress = (((index as f64) + stage_fraction).clamp(0.0, total_f)) / total_f;
    let width = 24;
    let filled = (progress * width as f64).round() as usize;
    let filled = filled.min(width);
    let bar = format!(
        "[{}{}]",
        "█".repeat(filled),
        " ".repeat(width.saturating_sub(filled))
    );
    write!(
        out,
        "\r{} {:>3}% {}",
        bar,
        (progress * 100.0) as u32,
        message
    )?;
    out.flush()
}

fn finish_progress_line(out: &mut dyn Write) -> io::Result<()> {
    #![allow(dead_code)]
    writeln!(out)
}

fn module_cli_actor(actor: Option<&AuditActor>) -> AuditActor {
    if let Some(actor) = actor {
        return actor.clone();
    }
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}

fn record_module_audit(
    deps: &CliDependencies,
    action: &str,
    module: &ModuleId,
    version: Option<&ModuleVersion>,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    let mut metadata = metadata.insert("transport", "cli").insert(
        "command",
        format!("modules {}", action.split("::").last().unwrap_or(action)),
    );

    if let Some(version) = version {
        metadata = metadata.insert("version", version.to_string());
    }

    let target = module.as_str().to_string();
    let actor = module_cli_actor(deps.session_actor());
    let event = AuditEvent::builder()
        .actor(actor)
        .action(action)
        .target(target)
        .outcome(outcome)
        .metadata(metadata)
        .build();

    match event {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                warn!(error = %err, "module audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "module audit build failed"),
    }
}

fn render_task_block(out: &mut dyn Write, module_label: &str, steps: &[(String, String)]) -> io::Result<()> {
    writeln!(out, "[TASK]   {}", module_label)?;
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

async fn run_synchronize_task(
    deps: CliDependencies,
    service: Arc<ModuleService>,
    module_id: ModuleId,
) -> io::Result<CommandOutcome> {
    let sink = deps
        .output()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no output sink available"))?;
    let mut out = OwnedStreamedWriter::new(sink);

    writeln!(&mut out, "\n[SYNC] {}", module_id)?;
    let result = service.synchronize_from_local(&module_id).await;
    match result {
        Ok(ModuleSyncOutcome::Packaged(package)) => {
            let version = ModuleVersion(package.install_result.manifest.version.clone());
            let mut steps = Vec::new();
            steps.push((
                "Packaging".to_string(),
                format!("[ OK ] {}", package.packaged_from.display()),
            ));
            steps.push((
                "Installing".to_string(),
                format!("install v{}", version),
            ));
            steps.push((
                "Starting".to_string(),
                "[ SKIP ] auto-managed".to_string(),
            ));
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;  // Leerzeile zwischen Task-Block und DONE
            writeln!(
                out,
                "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' synchronized.",
                module_id
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
            let mut steps = Vec::new();
            steps.push((
                "Switching".to_string(),
                "[ OK ] dev services".to_string(),
            ));
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;  // Leerzeile zwischen Task-Block und DONE
            writeln!(
                out,
                "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' synchronized.",
                module_id
            )?;
            record_module_audit(
                &deps,
                "module::synchronize",
                &module_id,
                Some(&dev_services.version),
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("mode", "dev_services")
                    .insert(
                        "service_ids",
                        dev_services
                            .services
                            .iter()
                            .map(|svc| svc.service_id.clone())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
            );
        }
        Err(err) => {
            render_service_error(&mut out, "Synchronisation fehlgeschlagen", &err)?;
            record_module_audit(
                &deps,
                "module::synchronize",
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

async fn run_release_task(
    deps: CliDependencies,
    service: Arc<ModuleService>,
    module_id: ModuleId,
    fenrir_version: String,
) -> io::Result<CommandOutcome> {
    let sink = deps
        .output()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no output sink available"))?;
    let mut out = OwnedStreamedWriter::new(sink);

    writeln!(&mut out, "\n[RELEASE] {}", module_id)?;
    let fenrir_version_for_call = fenrir_version.clone();
    let result = service
        .release_override(&module_id, &fenrir_version_for_call)
        .await;

    match result {
        Ok(install_result) => {
            let recorded_version = ModuleVersion(install_result.manifest.version.clone());
            let mut steps = Vec::new();
            steps.push((
                "Resetting".to_string(),
                "[ OK ] overrides".to_string(),
            ));
            steps.push((
                "Installing".to_string(),
                format!("install v{}", recorded_version),
            ));
            steps.push((
                "Starting".to_string(),
                "[ SKIP ] auto-managed".to_string(),
            ));
            render_task_block(&mut out, &module_id.to_string(), &steps)?;
            writeln!(out)?;  // Leerzeile zwischen Task-Block und DONE
            writeln!(
                &mut out,
                "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' restored to v{}.",
                module_id, recorded_version
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
            render_service_error(&mut out, "Release fehlgeschlagen", &err)?;
            record_module_audit(
                &deps,
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

fn render_distribution_plan(
    out: &mut dyn Write,
    plan: &[DistributionPlanEntry],
) -> io::Result<()> {
    writeln!(out, "\n:: DISTRIBUTION PLAN ::")?;
    let mut table = Table::new(vec![
        "Action".to_string(),
        "Module".to_string(),
        "Current".to_string(),
        "Target".to_string(),
    ]);

    for entry in plan {
        let action = match entry.action {
            DistributionAction::Install => "[+] Install",
            DistributionAction::Update => "[~] Update",
            DistributionAction::AlreadyCurrent => "[=] Current",
        };
        let current = entry
            .current_version
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(none)".to_string());
        table.add_row(vec![
            action.to_string(),
            entry.module_id.to_string(),
            current,
            entry.target_version.to_string(),
        ]);
    }

    table.render(out, "  ")?;
    writeln!(out)?;
    out.flush()
}

struct OwnedStreamedWriter {
    sink: Arc<dyn CommandOutput>,
}

impl OwnedStreamedWriter {
    fn new(sink: Arc<dyn CommandOutput>) -> Self {
        Self { sink }
    }
}

impl Write for OwnedStreamedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.push(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct InstallDistributionConfirmation {
    service: Arc<ModuleService>,
    plan: Vec<DistributionPlanEntry>,
    fenrir_version: String,
}

impl ConfirmationHandler for InstallDistributionConfirmation {
    fn handle(
        self: Box<Self>,
        accepted: bool,
        deps: &CliDependencies,
        out: &mut dyn Write,
    ) -> io::Result<CommandOutcome> {
        if !accepted {
            writeln!(out, "Installation abgebrochen.")?;
            return Ok(CommandOutcome::Continue);
        }

        // Run asynchronously to keep transports responsive; both CLI and SSH provide an output sink.
        let deps_clone = deps.clone();
        let future = async move { self.execute(&deps_clone).await };
        Ok(CommandOutcome::AsyncTask(Box::pin(future)))
    }
}

impl InstallDistributionConfirmation {
    async fn execute(
        self,
        deps: &CliDependencies,
    ) -> io::Result<CommandOutcome> {
        // Use owned writer for async context
        let sink = deps.output().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "no output sink available")
        })?;
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
                "Alle Module für Fenrir {} sind bereits aktuell.",
                self.fenrir_version
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
                            render_runtime_error(&mut out, "Stop fehlgeschlagen", &err)?;
                            return Ok(CommandOutcome::Continue);
                        }
                        steps.push(("Stopping".to_string(), "[ OK ] stopped".to_string()));
                        needs_restart = true;
                    }
                    Err(ModuleRuntimeError::NotRunning { .. }) => {}
                    Err(err) => {
                        render_runtime_error(
                            &mut out,
                            "Laufstatus konnte nicht ermittelt werden",
                            &err,
                        )?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            }

            let module_id_for_install = entry.module_id.clone();
            let target_version = entry.target_version.clone();
            let service_for_install = Arc::clone(&service);

            // Progress channel for live download updates
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<ModuleProgress>();
            let progress_callback: Arc<dyn Fn(ModuleProgress) + Send + Sync> =
                Arc::new(move |progress: ModuleProgress| {
                    let _ = progress_tx.send(progress);
                });

            let install_task = tokio::spawn(async move {
                service_for_install
                    .install_with_progress(&module_id_for_install, Some(&target_version), Some(progress_callback))
                    .await
            });

            // Render progress updates while install runs; keep last bar to embed in task block
            let mut last_percent = 0u32;
            let mut download_bar: Option<String> = None;
            let mut install_task = install_task;
            let install_result = loop {
                tokio::select! {
                    biased;
                    Some(progress) = progress_rx.recv() => {
                        if let ModuleProgress::DownloadProgress { downloaded_bytes, total_bytes, .. } = progress {
                            if let Some(total) = total_bytes {
                                let percent = ((downloaded_bytes as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u32;
                                if percent >= last_percent + 5 || percent == 100 {
                                    let mb_downloaded = downloaded_bytes as f64 / 1_048_576.0;
                                    let mb_total = total as f64 / 1_048_576.0;
                                    let bar_width = 20;
                                    let filled = ((percent as f64 / 100.0) * bar_width as f64).round() as usize;
                                    let filled = filled.min(bar_width);
                                    let bar = format!(
                                        "[{}{}]",
                                        "=".repeat(filled),
                                        ".".repeat(bar_width.saturating_sub(filled))
                                    );
                                    download_bar = Some(format!("{} {:>3}% ({:.1} MB / {:.1} MB)", bar, percent, mb_downloaded, mb_total));
                                    last_percent = percent;
                                }
                            }
                        }
                    }
                    result = &mut install_task => {
                        let result = result.map_err(|err| {
                            io::Error::new(io::ErrorKind::Other, format!("install task panicked: {err:?}"))
                        })?;
                        break result;
                    }
                }
            };

            match install_result {
                Ok(result) => {
                    if let Some(bar) = download_bar.take() {
                        steps.push(("Downloading".to_string(), bar));
                    }
                    let status_label = match result.status {
                        ModuleInstallStatus::Installed => "install v",
                        ModuleInstallStatus::Updated => "update to v",
                        ModuleInstallStatus::AlreadyCurrent => "current v",
                    };
                    steps.push((
                        "Installing".to_string(),
                        format!("{}{}", status_label, result.manifest.version),
                    ));
                    if matches!(result.status, ModuleInstallStatus::AlreadyCurrent) {
                        needs_restart = false;
                    }
                }
                Err(err) => {
                    render_service_error(
                        &mut out,
                        &format!("Installation von {} fehlgeschlagen", module_label),
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
                        let pid_hint = info
                            .pid
                            .map(|pid| format!("[ OK ] Service running (PID {pid})"))
                            .unwrap_or_else(|| "[ OK ] Service running".to_string());
                        steps.push(("Starting".to_string(), pid_hint));
                    }
                    Err(ModuleRuntimeError::AlreadyRunning { .. }) => steps.push((
                        "Starting".to_string(),
                        "[ SKIP ] already running".to_string(),
                    )),
                    Err(err) => {
                        render_runtime_error(&mut out, "Start fehlgeschlagen", &err)?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            } else {
                steps.push((
                    "Starting".to_string(),
                    "[ SKIP ] not required".to_string(),
                ));
            }

            render_task_block(&mut out, &module_label, &steps)?;
        }

        writeln!(out)?;
        writeln!(
            out,
            "[ \x1b[38;5;76mDONE\x1b[0m ] Distribution {} successfully applied.",
            self.fenrir_version
        )?;
        out.flush()?;

        Ok(CommandOutcome::Continue)
    }
}
