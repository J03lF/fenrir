pub mod command {
    pub const DESCRIPTION: &str = "Module management";
    pub const SYNOPSIS: &str = "modules <subcommand>";
    pub const DETAILS: &[&str] = &[
        "list modules                    – shows all modules with runtime status",
        "search modules [pattern]        – searches the registry",
        "show module <name[@version]>    – shows manifest information",
        "install distribution            – installs every module of a distribution",
        "synchronize module <name>       – replaces a module with local changes",
        "release module <name>           – returns a module to the distribution build",
        "release dev-overrides           – releases all modules currently in sync mode",
        "uninstall module <name>         – removes an installed module",
        "check modules                   – checks for available updates",
        "logs module <name> [--tail N]   – shows logs for a running module",
        "modules services [name]         – list module runtime/override services",
        "modules start <name>            – start module runtime",
        "modules stop <name>             – stop module runtime",
        "modules restart <name>          – restart module runtime",
        "stop-all modules                – stop every running module runtime",
    ];
}

pub mod search {
    pub const DESCRIPTION: &str = "Search signed modules inside the registry";
    pub const SYNOPSIS: &str = "search modules [pattern]";
    pub const DETAILS: &[&str] = &["search modules [pattern] – query the module registry"];
}

pub mod install_distribution {
    pub const DESCRIPTION: &str = "Install modules for a Fenrir distribution";
    pub const SYNOPSIS: &str = "install distribution";
    pub const DETAILS: &[&str] =
        &["install distribution – installs/updates all compatible modules"];
}

pub mod synchronize {
    pub const DESCRIPTION: &str = "Apply local module changes";
    pub const SYNOPSIS: &str = "synchronize module <name>";
    pub const DETAILS: &[&str] = &[
        "synchronize module <name> – replaces the module with local files",
        "Use [modules.dev_sources] base_path to automatically consume dev builds",
        ".fenrir-dev.toml can define dev service endpoints; sync module then registers those services instead of packaging artifacts",
    ];
}

pub mod release {
    pub const DESCRIPTION: &str = "Revert local module changes";
    pub const SYNOPSIS: &str = "release module <name>";
    pub const DETAILS: &[&str] = &["release module <name> – restore the distribution build"];
}

pub mod release_dev_overrides {
    pub const DESCRIPTION: &str = "Release every module that is in sync mode";
    pub const SYNOPSIS: &str = "release dev-overrides";
    pub const DETAILS: &[&str] =
        &["release dev-overrides – restore all modules that currently use sync overrides"];
    pub const USAGE: &str = "Use: release dev-overrides";
    pub const ERROR_CONTEXT: &str = "Failed to release dev overrides";
    pub const EMPTY_STATE: &str = "No modules are using sync overrides.";
    pub fn releasing(count: usize) -> String {
        format!("Releasing {count} dev override(s)...")
    }
}

pub mod uninstall {
    pub const DESCRIPTION: &str = "Remove installed modules";
    pub const SYNOPSIS: &str = "uninstall module <name>";
    pub const DETAILS: &[&str] = &["uninstall module <name> – remove a module"];
}

pub mod check {
    pub const DESCRIPTION: &str = "Check for module updates";
    pub const SYNOPSIS: &str = "check modules";
    pub const DETAILS: &[&str] = &["check modules – look for available module updates"];
}

pub mod logs {
    pub const DESCRIPTION: &str = "Show runtime logs for a module";
    pub const SYNOPSIS: &str = "logs module <name> [--tail N]";
    pub const DETAILS: &[&str] = &["logs module <name> [--tail N] – show runtime logs"];
}

pub mod stop_all {
    pub const DESCRIPTION: &str = "Stop all running modules";
    pub const SYNOPSIS: &str = "stop-all modules";
    pub const DETAILS: &[&str] = &["stop-all modules – stop every running module runtime"];
    pub const USAGE: &str = "Use: stop-all modules";
    pub const SUCCESS: &str = "All modules stopped.";
    pub const ERROR_CONTEXT: &str = "Failed to stop modules";
}

pub mod info {
    pub const DESCRIPTION: &str = "Show manifest information";
    pub const SYNOPSIS: &str = "show module <name[@version]>";
    pub const DETAILS: &[&str] = &["show module <name[@version]> – open the module manifest"];
}

pub mod subcommands {
    pub const LIST_DESC: &str = "List installed modules";
    pub const SEARCH_DESC: &str = "Search the registry for modules";
    pub const INFO_DESC: &str = "Show a module manifest";
    pub const INSTALL_DESC: &str = "Install all modules for a Fenrir distribution";
    pub const SYNCHRONIZE_DESC: &str = "Apply local changes for a module";
    pub const RELEASE_DESC: &str = "Revert a module to the distribution build";
    pub const RELEASE_DEV_OVERRIDES_DESC: &str =
        "Release every module that is currently in sync mode";
    pub const UNINSTALL_DESC: &str = "Uninstall a module";
    pub const CHECK_DESC: &str = "Check for available module updates";
    pub const LOGS_DESC: &str = "Show module logs";
    pub const SERVICES_DESC: &str = "List module services";
    pub const START_DESC: &str = "Start a module runtime";
    pub const STOP_DESC: &str = "Stop a module runtime";
    pub const RESTART_DESC: &str = "Restart a module runtime";
    pub const STOP_ALL_DESC: &str = "Stop all module runtimes";
}

pub mod routing {
    pub fn unknown_subcommand(name: &str) -> String {
        format!("Unknown subcommand '{name}'. Use 'help modules'.")
    }

    pub fn available_subcommands(list: &str) -> String {
        format!("Available subcommands: {list}")
    }

    pub fn unimplemented(name: &str) -> String {
        format!("Subcommand '{name}' is not implemented yet.")
    }
}

pub mod list_modules {
    pub const EXTRA_ARGS_WARNING: &str = "'list modules' does not accept additional arguments.";
    pub const LOAD_ERROR_CONTEXT: &str = "Failed to load installed modules";
    pub const TITLE: &str = "Installed modules";
    pub const UNDERLINE: &str = "====================";
    pub const EMPTY_STATE: &str = "(no modules installed)";
    pub const HEADERS: &[&str] = &[
        "Module", "Version", "Source", "Status", "PID", "Port", "Uptime",
    ];
    pub const STATUS_RUNNING: &str = "Running";
    pub const STATUS_STOPPED: &str = "Stopped";
    pub const STATUS_SYNC_SUFFIX: &str = "(sync)";
    pub const EMPTY_VALUE: &str = "-";
}

pub mod search_results {
    pub const LOAD_ERROR_CONTEXT: &str = "Registry search failed";
    pub const TITLE: &str = "Registry results";
    pub const UNDERLINE: &str = "==================";
    pub const EMPTY_STATE: &str = "No matches.";
    pub const HEADERS: &[&str] = &["Module", "Version", "Description"];
    pub const EMPTY_DESCRIPTION: &str = "-";
}

pub mod info_view {
    pub const USAGE: &str = "Use: show module <name[@version]>";
    pub const LOAD_ERROR_CONTEXT: &str = "Failed to load manifest";
}

pub mod install_flow {
    pub fn args_not_required(version: &str) -> String {
        format!("Fenrir version is detected automatically ({version}).")
    }
    pub const LOAD_ERROR_CONTEXT: &str = "Failed to determine distribution";
    pub fn not_found(version: &str) -> String {
        format!("No modules for Fenrir {version} found in the registry.")
    }
    pub fn already_installed(version: &str) -> String {
        format!("All modules for Fenrir {version} are already installed.")
    }
    pub const CONFIRM_PROMPT: &str = "? Apply distribution plan? [Y/n] ";
}

pub mod service {
    pub const UNAVAILABLE: &str = "Module service unavailable – please check boot logs.";
}

pub mod uninstall_flow {
    pub const USAGE: &str = "Use: uninstall module <name>";
    pub const ERROR_CONTEXT: &str = "Uninstall failed";
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
    pub const ERROR_CONTEXT: &str = "Synchronization failed";

    pub fn packaged_summary(module: &str, source: &str, target: &str) -> String {
        format!(
            "✓ Module {} synchronized with local files (source: {}, installed into {}).",
            module, source, target
        )
    }

    pub fn dev_services_header(module: &str) -> String {
        format!("✓ Module {} switched to dev service endpoints:", module)
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

    pub fn env_file_hint(service: &str, path: &str) -> String {
        format!(
            "   • Environment exports for {service} written to {path}. Source this file before running your module."
        )
    }

    pub fn env_file_error(err: &str) -> String {
        format!("   • Warning: failed to write dev env file ({err})")
    }

    pub fn dev_agent_started(
        command: &str,
        workdir: &str,
        log: &str,
        auto_restart: bool,
    ) -> String {
        let restart = if auto_restart {
            "auto-restart enabled"
        } else {
            "auto-restart disabled"
        };
        format!("   • Dev agent running `{command}` in {workdir} ({restart}). Logs → {log}.")
    }

    pub fn dev_agent_manual(command: &str, workdir: &str, env_hint: Option<&str>) -> String {
        match env_hint {
            Some(path) => format!(
                "   • Dev agent prepared env for `{command}` in {workdir}. Source {path} (or configure your IDE) before running `cargo run`."
            ),
            None => format!(
                "   • Dev agent prepared env for `{command}` in {workdir}. Use the generated env exports before running."
            ),
        }
    }

    pub const DEV_AGENT_NOT_CONFIGURED: &str =
        "   • No [dev.run] section found – start your module manually (see env exports).";
}

pub mod release_flow {
    pub const USAGE: &str = "Use: release module <name>";
    pub const ERROR_CONTEXT: &str = "Release failed";

    pub fn success(module: &str, fenrir_version: &str, version: &str) -> String {
        format!(
            "✓ Module {} is back on the distribution (Fenrir {}) v{}.",
            module, fenrir_version, version
        )
    }
}

pub mod check_flow {
    pub const NO_ARGS_WARNING: &str = "'check modules' expects no arguments.";
    pub const PROGRESS: &str = "Checking for updates...";
    pub const ERROR_CONTEXT: &str = "Update check failed";
    pub const NO_MODULES: &str = "No installed modules.";
    pub const HEADERS: &[&str] = &["Module", "Installed", "Available", "Status"];
    pub const STATUS_AVAILABLE: &str = "Update available";
    pub const STATUS_INCOMPATIBLE: &str = "Incompatible";
    pub const STATUS_CURRENT: &str = "Current";
    pub fn available_updates_hint(count: usize) -> String {
        format!(
            "💡 {} update(s) available. Use 'install distribution' to upgrade.",
            count
        )
    }
}

pub mod logs_flow {
    pub const USAGE: &str = "Use: logs module <name> [--tail N]";
    pub const TAIL_REQUIRES_VALUE: &str = "--tail requires a value";
    pub fn invalid_tail(value: &str) -> String {
        format!("Invalid tail value: {value}")
    }
    pub fn unknown_option(flag: &str) -> String {
        format!("Unknown option {flag}")
    }
    pub fn header(module: &str, line_count: usize) -> String {
        format!("Logs for module {} (lines: {})", module, line_count)
    }
    pub const SEPARATOR: &str = "========================================";
    pub const ERROR_CONTEXT: &str = "Log query failed";
}

pub mod services_view {
    pub const USAGE: &str = "Use: modules services [module]";
    pub const LOAD_ERROR_CONTEXT: &str = "Failed to inspect module services";
    pub const RUNTIME_STATUS_ERROR_CONTEXT: &str = "Failed to inspect module runtime state";
    pub const EMPTY_STATE: &str = "No module services registered.";
    pub fn empty_for_module(module: &str) -> String {
        format!("No services registered for module {module}.")
    }
    pub const HEADERS: &[&str] = &[
        "Module", "Service", "Type", "Status", "Since", "Route", "Endpoint", "Note",
    ];
    pub const EMPTY_VALUE: &str = "-";
    pub const TYPE_RUNTIME: &str = "runtime";
    pub const TYPE_DECLARED: &str = "declared";
    pub const TYPE_DEV: &str = "dev override";
    pub const TYPE_OTHER: &str = "service";
    pub const RUNTIME_SERVICE_LABEL: &str = "main";
    pub fn runtime_status_warning(err: &str) -> String {
        format!("⚠ runtime status unavailable: {err}. Port information may be incomplete.")
    }
}

pub mod lifecycle {
    pub const START_USAGE: &str = "Use: modules start <name>";
    pub const STOP_USAGE: &str = "Use: modules stop <name>";
    pub const RESTART_USAGE: &str = "Use: modules restart <name>";
    pub const START_ERROR_CONTEXT: &str = "Failed to start module";
    pub const STOP_ERROR_CONTEXT: &str = "Failed to stop module";
    pub const RESTART_ERROR_CONTEXT: &str = "Failed to restart module";

    pub fn start_success(module: &str, pid: Option<u32>, port: Option<u16>) -> String {
        match (pid, port) {
            (Some(pid), Some(port)) => {
                format!("✓ Module {module} started (pid {pid}, port {port}).")
            }
            (Some(pid), None) => format!("✓ Module {module} started (pid {pid})."),
            (None, Some(port)) => format!("✓ Module {module} started (port {port})."),
            _ => format!("✓ Module {module} started."),
        }
    }

    pub fn stop_success(module: &str) -> String {
        format!("✓ Module {module} stopped.")
    }

    pub fn restart_success(module: &str, pid: Option<u32>, port: Option<u16>) -> String {
        match (pid, port) {
            (Some(pid), Some(port)) => {
                format!("✓ Module {module} restarted (pid {pid}, port {port}).")
            }
            (Some(pid), None) => format!("✓ Module {module} restarted (pid {pid})."),
            (None, Some(port)) => format!("✓ Module {module} restarted (port {port})."),
            _ => format!("✓ Module {module} restarted."),
        }
    }
}

pub mod parser {
    pub fn invalid_module_id(raw: &str, err: &str) -> String {
        format!("Invalid module id '{}': {err}", raw)
    }

    pub const VERSION_REQUIRES_VALUE: &str = "--version requires a value";

    pub fn unknown_option(flag: &str) -> String {
        format!("Unknown option {flag}")
    }

    pub fn invalid_id_value(value: &str, err: &str) -> String {
        format!("Invalid module id `{value}`: {err}")
    }

    pub fn invalid_version(value: &str, err: &str) -> String {
        format!("Invalid version `{value}`: {err}")
    }
}

pub mod scoped_command {
    pub fn usage(usage: &str) -> String {
        format!("Usage: {usage}")
    }

    pub fn unknown_resource(resource: &str) -> String {
        format!("Unknown resource: {resource}")
    }
}

pub mod manifest_view {
    pub fn header(module_id: &str) -> String {
        format!("Manifest for module {}", module_id)
    }

    pub const UNDERLINE: &str = "===============================";

    pub fn version(value: &str) -> String {
        format!("Version: {value}")
    }

    pub fn title(value: &str) -> String {
        format!("Title: {value}")
    }

    pub fn description(value: &str) -> String {
        format!("Description: {value}")
    }

    pub fn license(value: &str) -> String {
        format!("License: {value}")
    }

    pub fn authors(value: &str) -> String {
        format!("Authors: {value}")
    }

    pub fn fenrir_version(value: &str) -> String {
        format!("Compatible with Fenrir: {value}")
    }

    pub fn tags(value: &str) -> String {
        format!("Tags: {value}")
    }

    pub fn download(url: &str) -> String {
        format!("Download: {url}")
    }

    pub fn checksum(value: &str) -> String {
        format!("Checksum: {value}")
    }

    pub fn signature_key(value: &str) -> String {
        format!("Signature key: {value}")
    }
}

pub mod distribution_plan_view {
    pub const TITLE: &str = "\n:: DISTRIBUTION PLAN ::";
    pub const HEADERS: &[&str] = &["Action", "Module", "Current", "Target", "Note"];
    pub const INSTALL_LABEL: &str = "[+] Install";
    pub const UPDATE_LABEL: &str = "[~] Update";
    pub const CURRENT_LABEL: &str = "[=] Current";
    pub const SKIP_LABEL: &str = "[x] Skip";
    pub const NO_CURRENT_VERSION: &str = "(none)";
    pub const SKIP_SYNC_NOTE: &str = "synchronized (skipped)";
}

pub mod service_errors {
    pub fn registry_unavailable(message: &str) -> String {
        format!("Registry unavailable: {message}")
    }

    pub fn registry_not_found(module: &str) -> String {
        format!("Module '{module}' was not found")
    }

    pub fn registry_protocol(message: &str) -> String {
        format!("Protocol error: {message}")
    }

    pub fn storage_unavailable(message: &str) -> String {
        message.to_string()
    }

    pub fn storage_invalid_state(message: &str) -> String {
        format!("Invalid state: {message}")
    }

    pub fn verification_message(message: &str) -> String {
        message.to_string()
    }

    pub const VERIFICATION_UNSUPPORTED: &str = "Signature algorithm is not supported";
}

pub mod runtime_errors {
    pub fn not_installed(module: &str) -> String {
        format!("Module '{module}' is not installed")
    }

    pub fn already_running(module: &str) -> String {
        format!("Module '{module}' is already running")
    }

    pub fn not_running(module: &str) -> String {
        format!("Module '{module}' is not running")
    }

    pub fn start_failed(module: &str, reason: &str) -> String {
        format!("Failed to start '{module}': {reason}")
    }

    pub fn stop_failed(module: &str, reason: &str) -> String {
        format!("Failed to stop '{module}': {reason}")
    }

    pub fn port_in_use(port: u16) -> String {
        format!("Port {port} is already in use")
    }

    pub fn no_available_ports(start: u16, end: u16) -> String {
        format!("No free module port in range {start}-{end}")
    }

    pub fn invalid_state(message: &str) -> String {
        format!("Invalid state: {message}")
    }

    pub fn io_error(message: &str) -> String {
        format!("I/O error: {message}")
    }

    pub fn quarantined(module: &str, until: &str) -> String {
        format!("Module '{module}' is quarantined until {until}")
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

    pub const INSTALL_ABORTED: &str = "Installation aborted.";

    pub fn fenrir_already_current(version: &str) -> String {
        format!("All modules for Fenrir {} are already up to date.", version)
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
        pub const STOP_FAILED: &str = "Stop failed";
        pub const STATUS_UNAVAILABLE: &str = "Runtime status could not be determined";
        pub const START_FAILED: &str = "Start failed";
    }

    pub mod service_contexts {
        pub fn installation_failed(module: &str) -> String {
            format!("Installation of {} failed", module)
        }
    }
}
