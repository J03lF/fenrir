use std::env::consts;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::infra::logging::{self, LogKind};
use tracing::{info, warn};

const DETAILS: &[&str] = &[
    "log              – streamt die aktuelle Applikationslogdatei",
    "log db           – streamt die DB-Logdatei",
    "log all          – öffnet App- und DB-Logs parallel",
    "log archive <ziel> – zeigt die letzte Archivdatei (ziel: app|db)",
    "log level <stufe> – setzt das Runtime-Loglevel (z. B. trace|debug|info|warn|error)",
];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "log",
        "Öffnet einen Log-Stream in einem neuen Terminal",
        "log [app|db|all|archive <ziel>|level <stufe>]",
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
    if args.is_empty() {
        info!(command = "log", target = "app", "log command invoked");
        display_log_result(out, "App", logging::log_file_path())?;
        return Ok(CommandOutcome::Continue);
    }

    let mut iter = args.iter().copied();
    let action = iter.next().unwrap_or("app");
    match action {
        "app" | "db" => {
            let kind = parse_target(action).unwrap();
            info!(command = "log", target = action, "log command invoked");
            let path = match kind {
                LogKind::App => logging::log_file_path(),
                LogKind::Db => logging::db_log_file_path(),
            };
            let label = match kind {
                LogKind::App => "App",
                LogKind::Db => "DB",
            };
            display_log_result(out, label, path)?;
        }
        "all" => {
            info!(command = "log", target = "all", "log command invoked");
            display_log_result(out, "App", logging::log_file_path())?;
            display_log_result(out, "DB", logging::db_log_file_path())?;
        }
        "level" => {
            let Some(level) = iter.next() else {
                writeln!(
                    out,
                    "fehlender Wert. Nutzung: log level <trace|debug|info|warn|error>"
                )?;
                return Ok(CommandOutcome::Continue);
            };
            if let Some(handle) = deps.services.logging_handle() {
                match logging::reload(&handle, level) {
                    Ok(()) => {
                        writeln!(out, "Loglevel aktualisiert auf '{level}'.")?;
                        info!(
                            command = "log",
                            mode = "level",
                            value = level,
                            "log level updated"
                        );
                    }
                    Err(err) => {
                        writeln!(out, "Konnte Loglevel nicht setzen: {err}")?;
                        tracing::warn!(error = %err, "failed to reload log level");
                    }
                }
            } else {
                writeln!(
                    out,
                    "Kein Logging-Reload-Handle vorhanden. SIGHUP oder CLI-Reload wird nicht unterstützt."
                )?;
            }
        }
        "archive" => {
            let target = iter.next().unwrap_or("app");
            if let Some(kind) = parse_target(target) {
                info!(
                    command = "log",
                    target = target,
                    mode = "archive",
                    "log command invoked"
                );
                let label = match kind {
                    LogKind::App => "App-Archiv",
                    LogKind::Db => "DB-Archiv",
                };
                let archive = logging::latest_archive(kind);
                display_log_result(out, label, archive)?;
            } else {
                writeln!(out, "unbekanntes Archiv-Ziel: {target} (erlaubt: app|db)")?;
            }
        }
        other => {
            warn!(command = "log", target = other, "unknown log subcommand");
            writeln!(
                out,
                "unbekanntes Ziel: {other}. Nutze 'log [app|db|all]', 'log archive [app|db]' oder 'log level <stufe>'."
            )?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn display_log_result(out: &mut dyn Write, label: &str, path: Option<PathBuf>) -> io::Result<()> {
    match path {
        Some(path) => {
            writeln!(out, "[{label}] Logdatei: {}", path.display())?;
            match launch_tail(&path) {
                Ok(()) => writeln!(out, "[{label}] Terminal wurde geöffnet.")?,
                Err(err) => writeln!(
                    out,
                    "[{label}] Konnte kein Terminal starten ({err}). Führe manuell aus: tail -n 200 -f \"{}\"",
                    path.display()
                )?,
            }
        }
        None => {
            warn!(category = label, "log path not configured");
            writeln!(out, "[{label}] Keine Logdatei verfügbar.")?
        }
    }
    Ok(())
}

fn launch_tail(path: &Path) -> io::Result<()> {
    match consts::OS {
        "macos" => launch_macos(path),
        "linux" => launch_linux(path),
        "windows" => launch_windows(path),
        _ => Err(io::Error::new(
            io::ErrorKind::Other,
            "keine unterstützte Terminal-Integration für dieses Betriebssystem",
        )),
    }
}

fn launch_macos(path: &std::path::Path) -> std::io::Result<()> {
    let command = format!(
        "tail -n 200 -f '{}'",
        escape_single_quotes(&path.display().to_string())
    );
    let cmd = escape_applescript(&command);

    // Erzwingt neuen Tab via Cmd+T und schreibt den Befehl genau in den ausgewählten Tab
    let script = format!(
        r#"
tell application "Terminal"
    activate
    if (count of windows) = 0 then
        -- Kein Fenster offen -> neues Fenster ist ok
        do script "{cmd}"
    else
        -- Sicher neuer TAB (nicht neues Fenster)
        tell application "System Events"
            if not (exists process "Terminal") then
                error "Terminal process not found"
            end if
            tell process "Terminal" to keystroke "t" using command down
        end tell
        delay 0.05
        do script "{cmd}" in selected tab of front window
    end if
end tell
"#,
        cmd = cmd
    );

    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map(|_| ())
}

fn launch_linux(path: &Path) -> io::Result<()> {
    let tail_command = format!(
        "tail -n 200 -f '{}'",
        escape_single_quotes(&path.display().to_string())
    );

    struct Launcher<'a> {
        bin: &'a str,
        args: &'a [&'a str],
        append_command: bool,
    }

    let attempts = [
        Launcher {
            bin: "gnome-terminal",
            args: &["--tab", "--", "bash", "-lc"],
            append_command: true,
        },
        Launcher {
            bin: "konsole",
            args: &["--new-tab", "bash", "-lc"],
            append_command: true,
        },
        Launcher {
            bin: "xfce4-terminal",
            args: &["--tab", "--", "bash", "-lc"],
            append_command: true,
        },
        Launcher {
            bin: "xterm",
            args: &["-e", "bash", "-lc"],
            append_command: true,
        },
    ];

    for launcher in attempts.iter() {
        let mut cmd = Command::new(launcher.bin);
        for arg in launcher.args {
            cmd.arg(arg);
        }
        if launcher.append_command {
            cmd.arg(&tail_command);
        }
        if cmd.spawn().is_ok() {
            info!(launcher = launcher.bin, path = %path.display(), "log tail terminal launched");
            return Ok(());
        }
    }

    warn!(path = %path.display(), "no supported terminal launcher found for tail");
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "kein unterstütztes Terminalprogramm gefunden (z. B. gnome-terminal, konsole)",
    ))
}

fn launch_windows(path: &Path) -> io::Result<()> {
    let ps_tail = format!("Get-Content -Path \"{}\" -Wait", path.display());
    let wt_args = [
        "-w",
        "0",
        "nt",
        "--title",
        "Fenrir Logs",
        "powershell",
        "-NoExit",
        "-Command",
        &ps_tail,
    ];
    if Command::new("wt").args(&wt_args).spawn().is_ok() {
        info!(launcher = "wt", path = %path.display(), "log tail terminal launched");
        return Ok(());
    }

    let ps_command = format!(
        "Start-Process powershell -ArgumentList '-NoExit','-Command','{}'",
        ps_tail.replace('"', "'"),
    );
    Command::new("powershell")
        .args(["-Command", &ps_command])
        .spawn()
        .map(|_| ())
}

fn escape_single_quotes(input: &str) -> String {
    input.replace("'", "'\\''")
}

fn escape_applescript(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_target(value: &str) -> Option<LogKind> {
    match value {
        "app" => Some(LogKind::App),
        "db" => Some(LogKind::Db),
        _ => None,
    }
}
