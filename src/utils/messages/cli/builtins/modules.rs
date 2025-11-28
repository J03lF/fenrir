pub mod command {
    pub const DESCRIPTION: &str = "Modulverwaltung";
    pub const SYNOPSIS: &str = "modules <subcommand>";
    pub const DETAILS: &[&str] = &[
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
}

pub mod search {
    pub const DESCRIPTION: &str = "Durchsucht signierte Module in der Registry";
    pub const SYNOPSIS: &str = "search modules [pattern]";
    pub const DETAILS: &[&str] = &["search modules [pattern] – durchsucht die Modul-Registry"];
}

pub mod install_distribution {
    pub const DESCRIPTION: &str = "Installiert Module einer Fenrir-Distribution";
    pub const SYNOPSIS: &str = "install distribution";
    pub const DETAILS: &[&str] =
        &["install distribution – installiert/aktualisiert alle kompatiblen Module"];
}

pub mod synchronize {
    pub const DESCRIPTION: &str = "Übernimmt lokale Moduländerungen";
    pub const SYNOPSIS: &str = "synchronize module <name>";
    pub const DETAILS: &[&str] = &[
        "synchronize module <name> – ersetzt das Modul durch lokale Dateien",
        "Nutze [modules.dev_sources] base_path, um Dev-Builds automatisch zu verwenden",
        ".fenrir-dev.toml kann Dev-Service-Endpunkte definieren; sync module registriert dann diese Services statt Artefakte zu packen",
    ];
}

pub mod release {
    pub const DESCRIPTION: &str = "Setzt lokale Moduländerungen zurück";
    pub const SYNOPSIS: &str = "release module <name>";
    pub const DETAILS: &[&str] =
        &["release module <name> – setzt lokale Anpassungen zurück zur Distribution"];
}

pub mod uninstall {
    pub const DESCRIPTION: &str = "Entfernt installierte Module";
    pub const SYNOPSIS: &str = "uninstall module <name>";
    pub const DETAILS: &[&str] = &["uninstall module <name> – entfernt ein Modul"];
}

pub mod check {
    pub const DESCRIPTION: &str = "Prüft verfügbare Modul-Updates";
    pub const SYNOPSIS: &str = "check modules";
    pub const DETAILS: &[&str] = &["check modules – prüft verfügbare Modul-Updates"];
}

pub mod logs {
    pub const DESCRIPTION: &str = "Zeigt Laufzeit-Logs eines Moduls";
    pub const SYNOPSIS: &str = "logs module <name> [--tail N]";
    pub const DETAILS: &[&str] = &["logs module <name> [--tail N] – zeigt Laufzeit-Logs"];
}

pub mod info {
    pub const DESCRIPTION: &str = "Zeigt Manifest-Informationen";
    pub const SYNOPSIS: &str = "show module <name[@version]>";
    pub const DETAILS: &[&str] = &["show module <name[@version]> – öffnet das Modul-Manifest"];
}

pub mod subcommands {
    pub const LIST_DESC: &str = "Installierte Module auflisten";
    pub const SEARCH_DESC: &str = "Registry nach Modulen durchsuchen";
    pub const INFO_DESC: &str = "Manifest eines Moduls anzeigen";
    pub const INSTALL_DESC: &str = "Alle Module für eine Fenrir-Distribution installieren";
    pub const SYNCHRONIZE_DESC: &str = "Lokale Änderungen eines Moduls übernehmen";
    pub const RELEASE_DESC: &str = "Modul auf Distribution zurücksetzen";
    pub const UNINSTALL_DESC: &str = "Modul deinstallieren";
    pub const CHECK_DESC: &str = "Verfügbare Modul-Updates prüfen";
    pub const LOGS_DESC: &str = "Modullogs anzeigen";
}

pub mod routing {
    pub fn unknown_subcommand(name: &str) -> String {
        format!("Unbekannter Subcommand '{name}'. Nutze 'help modules'.")
    }

    pub fn available_subcommands(list: &str) -> String {
        format!("Verfügbare Subcommands: {list}")
    }

    pub fn lifecycle_disabled(action: &str) -> String {
        format!("Modul-Lifecycle ist automatisiert – '{action}' module ist nicht mehr verfügbar.")
    }

    pub const LIFECYCLE_SUMMARY: &str =
        "Fenrir startet/stoppt Module beim Boot und während Distribution-Imports automatisch.";

    pub fn unimplemented(name: &str) -> String {
        format!("Subcommand '{name}' ist derzeit nicht implementiert.")
    }
}

pub mod list_modules {
    pub const EXTRA_ARGS_WARNING: &str = "'list modules' erwartet keine weiteren Argumente.";
    pub const LOAD_ERROR_CONTEXT: &str = "Installierte Module konnten nicht geladen werden";
    pub const TITLE: &str = "Installierte Module";
    pub const UNDERLINE: &str = "====================";
    pub const EMPTY_STATE: &str = "(keine Module installiert)";
    pub const HEADERS: &[&str] = &[
        "Modul", "Version", "Quelle", "Status", "PID", "Port", "Laufzeit",
    ];
    pub const STATUS_RUNNING: &str = "Running";
    pub const STATUS_STOPPED: &str = "Stopped";
    pub const EMPTY_VALUE: &str = "-";
}

pub mod search_results {
    pub const LOAD_ERROR_CONTEXT: &str = "Registry-Suche fehlgeschlagen";
    pub const TITLE: &str = "Registry-Ergebnisse";
    pub const UNDERLINE: &str = "==================";
    pub const EMPTY_STATE: &str = "Keine Treffer.";
    pub const HEADERS: &[&str] = &["Modul", "Version", "Beschreibung"];
    pub const EMPTY_DESCRIPTION: &str = "-";
}

pub mod info_view {
    pub const USAGE: &str = "Use: show module <name[@version]>";
    pub const LOAD_ERROR_CONTEXT: &str = "Manifest konnte nicht geladen werden";
}

pub mod install_flow {
    pub fn args_not_required(version: &str) -> String {
        format!("Fenrir-Version wird automatisch verwendet ({version}).")
    }
    pub const LOAD_ERROR_CONTEXT: &str = "Distribution konnte nicht ermittelt werden";
    pub fn not_found(version: &str) -> String {
        format!("Keine Module für Fenrir {version} in der Registry gefunden.")
    }
    pub fn already_installed(version: &str) -> String {
        format!("Alle Module für Fenrir {version} sind bereits installiert.")
    }
    pub const CONFIRM_PROMPT: &str = "? Apply distribution plan? [Y/n] ";
}

pub mod service {
    pub const UNAVAILABLE: &str = "Modul-Service nicht verfügbar – bitte Boot-Logs prüfen.";
}

pub mod uninstall_flow {
    pub const USAGE: &str = "Use: uninstall module <name>";
    pub const ERROR_CONTEXT: &str = "Deinstallation fehlgeschlagen";
    pub fn removal_header(module: &str) -> String {
        format!("\n[REMOVE] {module}")
    }
    pub const STOPPING_HOOKS: &str = "         ├── Stopping service hooks...     DONE";
    pub const UNLINKING_FILES: &str = "         ├── Unlinking module files...     DONE";
    pub const CLEANUP_CONFIGS: &str = "         └── Cleaning up configurations... DONE";
    pub fn removal_summary(module: &str) -> String {
        format!(
            "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' removed successfully.",
            module
        )
    }
}

pub mod synchronize_flow {
    pub const USAGE: &str = "Use: synchronize module <name>";
    pub const ERROR_CONTEXT: &str = "Synchronisation fehlgeschlagen";

    pub fn packaged_summary(module: &str, source: &str, target: &str) -> String {
        format!(
            "✓ Modul {} wurde mit lokalen Dateien synchronisiert (Quelle: {}, installiert nach {}).",
            module, source, target
        )
    }

    pub fn dev_services_header(module: &str) -> String {
        format!(
            "✓ Modul {} wurde auf Dev-Service-Endpunkte umgestellt:",
            module
        )
    }

    pub fn dev_service_entry(
        id: &str,
        endpoint: &str,
        name: &str,
        description: Option<&str>,
    ) -> String {
        match description {
            Some(desc) => format!("  - {} @ {} ({}) – {}", id, endpoint, name, desc),
            None => format!("  - {} @ {} ({})", id, endpoint, name),
        }
    }
}

pub mod release_flow {
    pub const USAGE: &str = "Use: release module <name>";
    pub const ERROR_CONTEXT: &str = "Release fehlgeschlagen";

    pub fn success(module: &str, fenrir_version: &str, version: &str) -> String {
        format!(
            "✓ Modul {} läuft wieder mit der Distribution (Fenrir {}) v{}.",
            module, fenrir_version, version
        )
    }
}

pub mod check_flow {
    pub const NO_ARGS_WARNING: &str = "'check modules' erwartet keine Argumente.";
    pub const PROGRESS: &str = "Prüfe Updates...";
    pub const ERROR_CONTEXT: &str = "Update-Prüfung fehlgeschlagen";
    pub const NO_MODULES: &str = "Keine installierten Module.";
    pub const HEADERS: &[&str] = &["Modul", "Installiert", "Verfügbar", "Status"];
    pub const STATUS_AVAILABLE: &str = "Update verfügbar";
    pub const STATUS_INCOMPATIBLE: &str = "Inkompatibel";
    pub const STATUS_CURRENT: &str = "Aktuell";
    pub fn available_updates_hint(count: usize) -> String {
        format!(
            "💡 {} Update(s) verfügbar. Nutze 'install distribution' zum Aktualisieren.",
            count
        )
    }
}

pub mod logs_flow {
    pub const USAGE: &str = "Use: logs module <name> [--tail N]";
    pub const TAIL_REQUIRES_VALUE: &str = "--tail benötigt einen Wert";
    pub fn invalid_tail(value: &str) -> String {
        format!("Ungültiger tail-Wert: {value}")
    }
    pub fn unknown_option(flag: &str) -> String {
        format!("Unbekannte Option {flag}")
    }
    pub fn header(module: &str, line_count: usize) -> String {
        format!("Logs für Modul {} (Zeilen: {})", module, line_count)
    }
    pub const SEPARATOR: &str = "========================================";
    pub const ERROR_CONTEXT: &str = "Log-Abfrage fehlgeschlagen";
}

pub mod parser {
    pub fn invalid_module_id(raw: &str, err: &str) -> String {
        format!("Ungültige Modul-ID '{}': {err}", raw)
    }

    pub const VERSION_REQUIRES_VALUE: &str = "--version benötigt einen Wert";

    pub fn unknown_option(flag: &str) -> String {
        format!("Unbekannte Option {flag}")
    }

    pub fn invalid_id_value(value: &str, err: &str) -> String {
        format!("Ungültige Modul-ID `{value}`: {err}")
    }

    pub fn invalid_version(value: &str, err: &str) -> String {
        format!("Ungültige Version `{value}`: {err}")
    }
}

pub mod scoped_command {
    pub fn usage(usage: &str) -> String {
        format!("Nutzung: {usage}")
    }

    pub fn unknown_resource(resource: &str) -> String {
        format!("Unbekannte Ressource: {resource}")
    }
}

pub mod manifest_view {
    pub fn header(module_id: &str) -> String {
        format!("Manifest für Modul {}", module_id)
    }

    pub const UNDERLINE: &str = "===============================";

    pub fn version(value: &str) -> String {
        format!("Version: {value}")
    }

    pub fn title(value: &str) -> String {
        format!("Titel: {value}")
    }

    pub fn description(value: &str) -> String {
        format!("Beschreibung: {value}")
    }

    pub fn license(value: &str) -> String {
        format!("Lizenz: {value}")
    }

    pub fn authors(value: &str) -> String {
        format!("Autoren: {value}")
    }

    pub fn fenrir_version(value: &str) -> String {
        format!("Kompatibel mit Fenrir: {value}")
    }

    pub fn tags(value: &str) -> String {
        format!("Tags: {value}")
    }

    pub fn download(url: &str) -> String {
        format!("Download: {url}")
    }

    pub fn checksum(value: &str) -> String {
        format!("Checksumme: {value}")
    }

    pub fn signature_key(value: &str) -> String {
        format!("Signatur-Schlüssel: {value}")
    }
}

pub mod distribution_plan_view {
    pub const TITLE: &str = "\n:: DISTRIBUTION PLAN ::";
    pub const HEADERS: &[&str] = &["Action", "Module", "Current", "Target"];
    pub const INSTALL_LABEL: &str = "[+] Install";
    pub const UPDATE_LABEL: &str = "[~] Update";
    pub const CURRENT_LABEL: &str = "[=] Current";
    pub const NO_CURRENT_VERSION: &str = "(none)";
}

pub mod service_errors {
    pub fn registry_unavailable(message: &str) -> String {
        format!("Registry nicht verfügbar: {message}")
    }

    pub fn registry_not_found(module: &str) -> String {
        format!("Modul '{module}' wurde nicht gefunden")
    }

    pub fn registry_protocol(message: &str) -> String {
        format!("Protokollfehler: {message}")
    }

    pub fn storage_unavailable(message: &str) -> String {
        message.to_string()
    }

    pub fn storage_invalid_state(message: &str) -> String {
        format!("Ungültiger Zustand: {message}")
    }

    pub fn verification_message(message: &str) -> String {
        message.to_string()
    }

    pub const VERIFICATION_UNSUPPORTED: &str = "Signaturalgorithmus wird nicht unterstützt";
}

pub mod runtime_errors {
    pub fn not_installed(module: &str) -> String {
        format!("Modul '{module}' ist nicht installiert")
    }

    pub fn already_running(module: &str) -> String {
        format!("Modul '{module}' läuft bereits")
    }

    pub fn not_running(module: &str) -> String {
        format!("Modul '{module}' läuft nicht")
    }

    pub fn start_failed(module: &str, reason: &str) -> String {
        format!("Start von '{module}' fehlgeschlagen: {reason}")
    }

    pub fn stop_failed(module: &str, reason: &str) -> String {
        format!("Stop von '{module}' fehlgeschlagen: {reason}")
    }

    pub fn port_in_use(port: u16) -> String {
        format!("Port {port} ist bereits belegt")
    }

    pub fn invalid_state(message: &str) -> String {
        format!("Ungültiger Zustand: {message}")
    }

    pub fn io_error(message: &str) -> String {
        format!("I/O-Fehler: {message}")
    }
}

pub mod error_wrappers {
    pub fn formatted(context: &str, message: &str, code: &str) -> String {
        format!("✗ {context}: {message} ({code})")
    }
}

pub mod tasks_flow {
    pub fn block_header(label: &str) -> String {
        format!("[TASK]   {label}")
    }

    pub fn sync_header(module: &str) -> String {
        format!("\n[SYNC] {module}")
    }

    pub fn release_header(module: &str) -> String {
        format!("\n[RELEASE] {module}")
    }

    pub fn sync_done(module: &str) -> String {
        format!(
            "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' synchronized.",
            module
        )
    }

    pub fn release_done(module: &str, version: &str) -> String {
        format!(
            "[ \x1b[38;5;76mDONE\x1b[0m ] Module '{}' restored to v{}.",
            module, version
        )
    }

    pub const INSTALL_ABORTED: &str = "Installation abgebrochen.";

    pub fn fenrir_already_current(version: &str) -> String {
        format!("Alle Module für Fenrir {} sind bereits aktuell.", version)
    }

    pub fn distribution_applied(version: &str) -> String {
        format!(
            "[ \x1b[38;5;76mDONE\x1b[0m ] Distribution {} successfully applied.",
            version
        )
    }

    pub mod steps {
        pub const PACKAGING: &str = "Packaging";
        pub const INSTALLING: &str = "Installing";
        pub const STARTING: &str = "Starting";
        pub const SWITCHING: &str = "Switching";
        pub const RESETTING: &str = "Resetting";
        pub const STOPPING: &str = "Stopping";
        pub const DOWNLOADING: &str = "Downloading";
    }

    pub mod details {
        pub fn packaging_ok(path: &str) -> String {
            format!("[ OK ] {path}")
        }

        pub fn installing_version(prefix: &str, version: &str) -> String {
            format!("{prefix}{version}")
        }

        pub const START_AUTO_MANAGED: &str = "[ SKIP ] auto-managed";
        pub const SWITCH_DEV_SERVICES: &str = "[ OK ] dev services";
        pub const RESET_OVERRIDES: &str = "[ OK ] overrides";
        pub fn install_version(version: &str) -> String {
            format!("install v{version}")
        }
        pub fn update_version(version: &str) -> String {
            format!("update to v{version}")
        }
        pub fn current_version(version: &str) -> String {
            format!("current v{version}")
        }
        pub const START_ALREADY_RUNNING: &str = "[ SKIP ] already running";
        pub const START_NOT_REQUIRED: &str = "[ SKIP ] not required";
        pub const STOPPED_OK: &str = "[ OK ] stopped";
        pub fn start_running(pid: Option<u32>) -> String {
            match pid {
                Some(pid) => format!("[ OK ] Service running (PID {pid})"),
                None => "[ OK ] Service running".to_string(),
            }
        }
    }

    pub mod runtime_contexts {
        pub const STOP_FAILED: &str = "Stop fehlgeschlagen";
        pub const STATUS_UNAVAILABLE: &str = "Laufstatus konnte nicht ermittelt werden";
        pub const START_FAILED: &str = "Start fehlgeschlagen";
    }

    pub mod service_contexts {
        pub fn installation_failed(module: &str) -> String {
            format!("Installation von {} fehlgeschlagen", module)
        }
    }
}
