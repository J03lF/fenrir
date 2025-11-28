use std::env::consts;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::infra::logging::LogKind;
use crate::utils::messages::cli::builtins::log::launch as log_launch_messages;
use tracing::{info, warn};

pub fn display_log_result(
    out: &mut dyn Write,
    label: &str,
    path: Option<PathBuf>,
) -> io::Result<()> {
    match path {
        Some(path) => {
            writeln!(out, "{}", log_launch_messages::log_file_line(label, &path))?;
            match launch_tail(&path) {
                Ok(()) => writeln!(out, "{}", log_launch_messages::terminal_opened(label))?,
                Err(err) => writeln!(
                    out,
                    "{}",
                    log_launch_messages::terminal_failed(label, &path, &err)
                )?,
            }
        }
        None => {
            warn!(category = label, "log path not configured");
            writeln!(out, "{}", log_launch_messages::no_log_available(label))?
        }
    }
    Ok(())
}

fn launch_tail(path: &Path) -> io::Result<()> {
    match consts::OS {
        "macos" => launch_macos(path),
        "linux" => launch_linux(path),
        "windows" => launch_windows(path),
        _ => Err(io::Error::other(log_launch_messages::UNSUPPORTED_OS)),
    }
}

fn launch_macos(path: &Path) -> io::Result<()> {
    let command = format!(
        "tail -n 200 -f '{}'",
        escape_single_quotes(&path.display().to_string())
    );
    let cmd = escape_applescript(&command);

    let script = format!(
        r#"
tell application "Terminal"
    activate
    if (count of windows) = 0 then
        do script "{cmd}"
    else
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

    Command::new("osascript")
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
        log_launch_messages::NO_TERMINAL_LAUNCHER,
    ))
}

fn launch_windows(path: &Path) -> io::Result<()> {
    let ps_tail = format!("Get-Content -Path \"{}\" -Wait", path.display());
    let wt_args = [
        "-w",
        "0",
        "nt",
        "--title",
        log_launch_messages::WINDOWS_TAB_TITLE,
        "powershell",
        "-NoExit",
        "-Command",
        &ps_tail,
    ];
    if Command::new("wt").args(wt_args).spawn().is_ok() {
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

pub fn parse_target(value: &str) -> Option<LogKind> {
    match value {
        "app" => Some(LogKind::App),
        "db" => Some(LogKind::Db),
        _ => None,
    }
}
