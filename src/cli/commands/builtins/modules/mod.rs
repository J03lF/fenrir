use std::future::Future;
use std::io::{self, Write};
use std::sync::Arc;
use tokio::runtime::{Handle, Runtime};
use tracing::{info, warn};
use whoami;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::module::{
    ModuleId, ModuleInstallStatus, ModuleManifest, ModuleRegistryError, ModuleSearchQuery,
    ModuleServiceError, ModuleStorageError, ModuleVerificationError, ModuleVersion,
};
use crate::services::ModuleService;
use crate::services::ServiceKind;
use crate::utils;

const DETAILS: &[&str] = &[
    "modules list                       – zeigt installierte Module",
    "modules search [muster]            – durchsucht Registry",
    "modules info <name[@version]>      – zeigt Manifestinformationen",
    "modules install <name[@version]>   – lädt, prüft und installiert ein Modul",
];

fn run_module_future<F, Fut, T>(factory: F) -> Result<T, ModuleServiceError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ModuleServiceError>> + Send + 'static,
    T: Send + 'static,
{
    if Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            Runtime::new()
                .map_err(|err| {
                    ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                        "runtime init failed: {err}"
                    )))
                })?
                .block_on(factory())
        })
        .join()
        .unwrap_or_else(|err| {
            Err(ModuleServiceError::Storage(
                ModuleStorageError::Unavailable(format!("blocking thread panicked: {err:?}")),
            ))
        });
    }
    Runtime::new()
        .map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::Unavailable(format!(
                "runtime init failed: {err}"
            )))
        })?
        .block_on(factory())
}

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "modules",
        "Modulverwaltung",
        "modules <subcommand>",
        DETAILS,
        handle,
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
    let outcome = match args.first().copied() {
        None | Some("list") => {
            list_installed(Arc::clone(&service), out)?;
            render_service_overview(out, deps)?;
            Ok(())
        }
        Some("search") => {
            let pattern = args.get(1).map(|value| value.to_string());
            search_registry(Arc::clone(&service), pattern, out)
        }
        Some("info") => {
            let target = args.get(1).copied();
            module_info(Arc::clone(&service), out, target)
        }
        Some("install") => install_module(Arc::clone(&service), deps, out, &args[1..]),
        Some(other) => {
            writeln!(
                out,
                "Unbekannter Subcommand '{other}'. Nutze 'modules list' oder 'help modules'."
            )?;
            Ok(())
        }
    };
    if let Err(err) = outcome {
        writeln!(out, "Fehler: {err}")?;
    }
    Ok(CommandOutcome::Continue)
}

fn list_installed(service: Arc<ModuleService>, out: &mut dyn Write) -> io::Result<()> {
    let result = run_module_future(move || {
        let service = Arc::clone(&service);
        async move { service.list_installed().await }
    });
    let modules = match result {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(
                out,
                "Installierte Module konnten nicht geladen werden",
                &err,
            )?;
            return Ok(());
        }
    };
    writeln!(out, "Installierte Module")?;
    writeln!(out, "====================")?;
    if modules.is_empty() {
        writeln!(out, "(keine Module installiert)")?;
        return Ok(());
    }
    let mut table = Table::new(vec![
        "Module".to_string(),
        "Version".to_string(),
        "Seit".to_string(),
        "Pfad".to_string(),
    ]);
    for module in modules {
        let duration = module
            .installed_at
            .elapsed()
            .ok()
            .map(utils::format_brief_duration)
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![
            module.manifest.id.clone(),
            module.manifest.version.to_string(),
            duration,
            module.path.clone(),
        ]);
    }
    table.render(out, "  ")
}

fn search_registry(
    service: Arc<ModuleService>,
    pattern: Option<String>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let result = run_module_future(move || {
        let service = Arc::clone(&service);
        let query = ModuleSearchQuery::new(pattern);
        async move { service.search(query).await }
    });
    let modules = match result {
        Ok(modules) => modules,
        Err(err) => {
            render_service_error(out, "Registry-Suche fehlgeschlagen", &err)?;
            return Ok(());
        }
    };
    writeln!(out, "Registry-Ergebnisse")?;
    writeln!(out, "==================")?;
    if modules.is_empty() {
        writeln!(out, "Keine Treffer.")?;
        return Ok(());
    }
    let mut table = Table::new(vec![
        "Module".to_string(),
        "Version".to_string(),
        "Titel".to_string(),
        "Tags".to_string(),
    ]);
    for entry in modules {
        let tags = if entry.tags.is_empty() {
            "-".to_string()
        } else {
            entry.tags.join(",")
        };
        table.add_row(vec![
            entry.id.to_string(),
            entry.version.to_string(),
            entry.title.unwrap_or_else(|| "-".to_string()),
            tags,
        ]);
    }
    table.render(out, "  ")
}

fn module_info(
    service: Arc<ModuleService>,
    out: &mut dyn Write,
    target: Option<&str>,
) -> io::Result<()> {
    let Some(target) = target else {
        writeln!(out, "Kommando: modules info <name[@version]>")?;
        return Ok(());
    };
    let (module_id, version) = match parse_module_target(target, &[]) {
        Ok(tuple) => tuple,
        Err(err) => {
            writeln!(out, "{err}")?;
            return Ok(());
        }
    };
    let result = run_module_future(move || {
        let service = Arc::clone(&service);
        let module_id = module_id.clone();
        let version = version.clone();
        async move { service.manifest(&module_id, version.as_ref()).await }
    });
    match result {
        Ok(manifest) => render_manifest(out, &manifest),
        Err(err) => render_service_error(out, "Manifest konnte nicht geladen werden", &err),
    }
}

fn install_module(
    service: Arc<ModuleService>,
    deps: &CliDependencies,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<()> {
    if args.is_empty() {
        writeln!(
            out,
            "Kommando: modules install <name[@version]> [--version <semver>]"
        )?;
        return Ok(());
    }
    let (module_id, version) = match parse_module_target(args[0], &args[1..]) {
        Ok(tuple) => tuple,
        Err(err) => {
            writeln!(out, "{err}")?;
            return Ok(());
        }
    };
    let module_id_for_call = module_id.clone();
    let version_for_call = version.clone();
    let install_result = run_module_future(move || {
        let service = Arc::clone(&service);
        async move {
            service
                .install(&module_id_for_call, version_for_call.as_ref())
                .await
        }
    });
    match install_result {
        Ok(result) => {
            let status_message = match result.status {
                ModuleInstallStatus::Installed => "installiert",
                ModuleInstallStatus::Updated => "aktualisiert",
                ModuleInstallStatus::AlreadyCurrent => "bereits aktuell",
            };
            writeln!(
                out,
                "Modul {} ({}) {} – Pfad: {}",
                result.manifest.id, result.manifest.version, status_message, result.path
            )?;
            let reported_version = version
                .clone()
                .unwrap_or_else(|| ModuleVersion(result.manifest.version.clone()));
            record_module_audit(
                deps,
                "module::install",
                &module_id,
                Some(&reported_version),
                AuditOutcome::Success,
                AuditMetadata::default()
                    .insert("status", status_message)
                    .insert("path", result.path),
            );
        }
        Err(err) => {
            render_service_error(out, "Installation fehlgeschlagen", &err)?;
            record_module_audit(
                deps,
                "module::install",
                &module_id,
                version.as_ref(),
                AuditOutcome::Failure,
                AuditMetadata::default()
                    .insert("error_code", module_error_code(&err))
                    .insert("error", err.to_string()),
            );
        }
    }
    Ok(())
}

fn render_service_overview(out: &mut dyn Write, deps: &CliDependencies) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "Registrierte Services:")?;
    let mut services = deps.services.registry().snapshot();
    if services.is_empty() {
        writeln!(out, "  (keine Services registriert)")?;
        return Ok(());
    }
    services.sort_by(|a, b| {
        let kind_cmp = kind_label(a.descriptor.kind).cmp(kind_label(b.descriptor.kind));
        if kind_cmp == std::cmp::Ordering::Equal {
            a.descriptor.name.cmp(b.descriptor.name)
        } else {
            kind_cmp
        }
    });
    let mut table = Table::new(vec![
        "Service".to_string(),
        "Typ".to_string(),
        "Status".to_string(),
        "Seit".to_string(),
        "Hinweis".to_string(),
    ]);
    for snapshot in services {
        let service_name = format!("{} ({})", snapshot.descriptor.name, snapshot.descriptor.id);
        let kind = kind_label(snapshot.descriptor.kind).to_string();
        let status = snapshot.status.label().to_string();
        let since = snapshot
            .since
            .elapsed()
            .ok()
            .map(utils::format_brief_duration)
            .unwrap_or_else(|| "-".to_string());
        let note = snapshot
            .note
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![service_name, kind, status, since, note]);
    }
    table.render(out, "  ")
}

fn kind_label(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Infrastructure => "Infrastructure",
        ServiceKind::Transport => "Transport",
        ServiceKind::BackgroundJob => "Background Jobs",
        ServiceKind::Cli => "CLI",
        ServiceKind::Security => "Security",
        ServiceKind::Storage => "Storage",
        ServiceKind::Other => "Other",
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
        "{context}: {} ({})",
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
            ModuleRegistryError::Unavailable(msg) => msg.clone(),
            ModuleRegistryError::NotFound { module } => {
                format!("Modul {module} wurde nicht gefunden")
            }
            ModuleRegistryError::Protocol(msg) => msg.clone(),
        },
        ModuleServiceError::Storage(inner) => match inner {
            ModuleStorageError::Unavailable(msg) | ModuleStorageError::Io(msg) => msg.clone(),
            ModuleStorageError::InvalidState(msg) => msg.clone(),
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
    let target = format!("{}", module.as_str());
    let event = AuditEvent::builder()
        .actor(module_cli_actor())
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

fn module_cli_actor() -> AuditActor {
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}
