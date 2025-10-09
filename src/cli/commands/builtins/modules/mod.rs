use std::future::Future;
use std::io::{self, Write};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;
use tokio::runtime::{Handle, Runtime};
use tracing::{info, warn};
use whoami;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::module::{
    ModuleId, ModuleInstallStatus, ModuleManifest, ModuleRegistryError, ModuleServiceError,
    ModuleStorageError, ModuleVerificationError, ModuleVersion,
};
use crate::services::ModuleService;
use crate::utils;

const DETAILS: &[&str] = &[
    "modules list                     – zeigt installierte Module",
    "modules search [pattern]         – durchsucht Registry",
    "modules info <name[@version]>    – zeigt Manifest-Informationen",
    "modules install <name[@version]> – installiert/aktualisiert Modul",
    "modules uninstall <name>         – entfernt ein installiertes Modul",
    "modules update [name]            – aktualisiert Modul(e)",
    "modules check-updates            – prüft verfügbare Updates",
];

// Helper to run async code from sync command handler
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
            Ok(())
        }
        Some("search") => {
            let pattern = args.get(1).map(|v| v.to_string());
            search_registry(Arc::clone(&service), pattern, out)
        }
        Some("info") => {
            let target = args.get(1).copied();
            module_info(Arc::clone(&service), out, target)
        }
        Some("install") => install_module(Arc::clone(&service), deps, out, &args[1..]),
        Some("uninstall") => uninstall_module(Arc::clone(&service), deps, out, &args[1..]),
        Some("update") => update_modules(Arc::clone(&service), deps, out, &args[1..]),
        Some("check-updates") => check_updates(Arc::clone(&service), deps, out),
        Some(other) => {
            writeln!(
                out,
                "Unbekannter Subcommand '{other}'. Nutze 'help modules'."
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
        "Modul".to_string(),
        "Version".to_string(),
        "Installiert".to_string(),
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
            format!("vor {}", duration),
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
    use crate::domain::module::ModuleSearchQuery;

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
        writeln!(out, "Kommando: modules install <name[@version]>")?;
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

    let spinner = CliSpinner::start(format!("Lade Modul {} herunter", module_id));

    let install_result = run_module_future(move || {
        let service = Arc::clone(&service);
        async move {
            service
                .install(&module_id_for_call, version_for_call.as_ref())
                .await
        }
    });

    spinner.finish();

    match install_result {
        Ok(result) => {
            writeln!(out, "Download abgeschlossen.")?;
            let status_message = match result.status {
                ModuleInstallStatus::Installed => "installiert",
                ModuleInstallStatus::Updated => "aktualisiert",
                ModuleInstallStatus::AlreadyCurrent => "bereits aktuell",
            };

            writeln!(
                out,
                "✓ Modul {} ({}) {} – Pfad: {}",
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
            writeln!(out, "Download fehlgeschlagen.")?;
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

struct CliSpinner {
    sender: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl CliSpinner {
    fn start(message: impl Into<String>) -> Self {
        let message = message.into();
        let (sender, receiver) = mpsc::channel();

        let handle = thread::spawn(move || {
            let frames = ['|', '/', '-', '\\'];
            let mut index = 0;
            let mut stdout = io::stdout();

            loop {
                match receiver.recv_timeout(Duration::from_millis(120)) {
                    Ok(_) => {
                        let width = message.len() + 2;
                        let _ = write!(stdout, "\r{:<width$}\r", "", width = width);
                        let _ = stdout.flush();
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let frame = frames[index % frames.len()];
                        index = (index + 1) % frames.len();
                        let _ = write!(stdout, "\r{} {}", message, frame);
                        let _ = stdout.flush();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let width = message.len() + 2;
                        let _ = write!(stdout, "\r{:<width$}\r", "", width = width);
                        let _ = stdout.flush();
                        break;
                    }
                }
            }
        });

        Self {
            sender: Some(sender),
            handle: Some(handle),
        }
    }

    fn finish(mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CliSpinner {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn uninstall_module(
    service: Arc<ModuleService>,
    deps: &CliDependencies,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<()> {
    if args.len() != 1 {
        writeln!(out, "Kommando: modules uninstall <name>")?;
        return Ok(());
    }

    let module_id = match ModuleId::new(args[0]) {
        Ok(id) => id,
        Err(err) => {
            writeln!(out, "Ungültige Modul-ID '{}': {}", args[0], err)?;
            return Ok(());
        }
    };

    let module_id_for_call = module_id.clone();
    let result = run_module_future(move || {
        let service = Arc::clone(&service);
        async move { service.uninstall(&module_id_for_call).await }
    });

    match result {
        Ok(_) => {
            writeln!(out, "✓ Modul '{}' wurde entfernt.", module_id)?;
            record_module_audit(
                deps,
                "module::uninstall",
                &module_id,
                None,
                AuditOutcome::Success,
                AuditMetadata::default(),
            );
        }
        Err(err) => {
            render_service_error(out, "Deinstallation fehlgeschlagen", &err)?;
            record_module_audit(
                deps,
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

    Ok(())
}

fn update_modules(
    service: Arc<ModuleService>,
    deps: &CliDependencies,
    out: &mut dyn Write,
    args: &[&str],
) -> io::Result<()> {
    let fenrir_version_str = deps.config.app.version.clone();

    if let Some(module_name) = args.first() {
        // Update specific module
        let module_id = match ModuleId::new(*module_name) {
            Ok(id) => id,
            Err(err) => {
                writeln!(out, "Ungültige Modul-ID: {}", err)?;
                return Ok(());
            }
        };

        let module_id_clone = module_id.clone();
        let fenrir_ver = fenrir_version_str.clone();
        let result = run_module_future(move || {
            let service = Arc::clone(&service);
            async move { service.update(&module_id_clone, Some(&fenrir_ver)).await }
        });

        match result {
            Ok(install_result) => {
                writeln!(
                    out,
                    "✓ Modul {} auf Version {} aktualisiert",
                    install_result.manifest.id, install_result.manifest.version
                )?;
            }
            Err(err) => {
                render_service_error(out, "Update fehlgeschlagen", &err)?;
            }
        }
    } else {
        // Update all modules
        writeln!(out, "Aktualisiere alle Module...")?;

        let fenrir_ver = fenrir_version_str.clone();
        let result = run_module_future(move || {
            let service = Arc::clone(&service);
            async move { service.update_all(Some(&fenrir_ver)).await }
        });

        match result {
            Ok(results) => {
                if results.is_empty() {
                    writeln!(out, "Keine Updates verfügbar.")?;
                } else {
                    writeln!(out, "{} Module wurden aktualisiert:", results.len())?;
                    for result in results {
                        writeln!(
                            out,
                            "  ✓ {} v{}",
                            result.manifest.id, result.manifest.version
                        )?;
                    }
                }
            }
            Err(err) => {
                render_service_error(out, "Update fehlgeschlagen", &err)?;
            }
        }
    }

    Ok(())
}

fn check_updates(
    service: Arc<ModuleService>,
    deps: &CliDependencies,
    out: &mut dyn Write,
) -> io::Result<()> {
    let fenrir_version = deps.config.app.version.clone();

    writeln!(out, "Prüfe Updates...")?;

    let result = run_module_future(move || {
        let service = Arc::clone(&service);
        let fenrir_ver = fenrir_version.clone();
        async move { service.check_updates(Some(&fenrir_ver)).await }
    });

    let updates = match result {
        Ok(updates) => updates,
        Err(err) => {
            render_service_error(out, "Update-Prüfung fehlgeschlagen", &err)?;
            return Ok(());
        }
    };

    if updates.is_empty() {
        writeln!(out, "Keine installierten Module.")?;
        return Ok(());
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
            "💡 {} Update(s) verfügbar. Nutze 'modules update' zum Aktualisieren.",
            available_updates
        )?;
    }

    Ok(())
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
