use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "fenrir")]
#[command(about = "Fenrir: secure ticketing monolith", long_about = None)]
struct Args {
    /// Validate configuration and exit
    #[arg(long)]
    check_config: bool,

    /// Start interactive CLI shell
    #[arg(long)]
    cli: bool,

    /// Execute a single command and exit
    #[arg(long)]
    command: Option<String>,

    /// Internal: run the dev agent supervisor
    #[arg(long, hide = true, value_name = "PATH")]
    dev_agent_config: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse_from(normalize_args());

    if let Some(config_path) = args.dev_agent_config {
        if let Err(err) = fenrir::dev_agent::run(config_path).await {
            eprintln!("dev agent failed: {err}");
            std::process::exit(1);
        }
        return;
    }

    if args.check_config {
        handle_check_config();
    }

    // Boot sequence (logging/telemetry)
    let ctx = match fenrir::boot::boot() {
        Ok(ctx) => ctx,
        Err(err) => {
            report_boot_error(&err);
            std::process::exit(1);
        }
    };

    // Old plugin init removed - modules are now managed via module service

    if let Some(command) = args.command {
        let mut out = std::io::stdout();
        let registry = fenrir::cli::commands::builtins::build_registry();
        let deps = fenrir::cli::commands::registry::CliDependencies::new(
            Arc::clone(&ctx.config),
            Arc::clone(&ctx.services),
        );
        let env = fenrir::cli::commands::registry::ShellEnvironment::Cli;
        let mut cmd_args: Vec<&str> = command.split_whitespace().collect();
        let cmd_name = cmd_args.remove(0);

        match registry.execute(cmd_name, &cmd_args, &deps, &mut out, env) {
            Ok(fenrir::cli::commands::registry::CommandStatus::Executed(
                fenrir::cli::commands::registry::CommandOutcome::AwaitConfirmation(_),
            )) => {
                eprintln!("Befehl '{cmd_name}' erfordert eine interaktive Bestätigung.");
                std::process::exit(2);
            }
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("Command execution failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    println!(
        "{} v{} started",
        ctx.config.app.name, ctx.config.app.version
    );
    if let Err(e) = fenrir::boot::start_transports(&ctx).await {
        eprintln!("transport init failed: {e}");
    }

    if args.cli {
        if let Err(e) =
            fenrir::cli::shell::run_shell(Arc::clone(&ctx.config), Arc::clone(&ctx.services))
        {
            eprintln!("cli error: {e}");
        }
    } else {
        // Keep running if started without CLI to serve SSH
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn normalize_args() -> Vec<String> {
    std::env::args()
        .map(|arg| {
            if arg == "-check-config" {
                "--check-config".to_string()
            } else {
                arg
            }
        })
        .collect()
}

fn handle_check_config() {
    match fenrir::config::load() {
        Ok(_) => {
            println!("CFG-OK configuration valid");
            std::process::exit(0);
        }
        Err(err) => {
            let code = config_error_code(&err);
            eprintln!("{code}: {err}");
            std::process::exit(1);
        }
    }
}

fn report_boot_error(err: &fenrir::boot::BootError) {
    if let Some(source) = <fenrir::boot::BootError as std::error::Error>::source(err) {
        eprintln!("{}: {} ({source})", err.code(), err.message());
    } else {
        eprintln!("{}: {}", err.code(), err.message());
    }
}

fn config_error_code(err: &fenrir::config::ConfigError) -> &'static str {
    match err {
        fenrir::config::ConfigError::MissingEnv { .. } => "CFG-MISSING-SECRET",
        fenrir::config::ConfigError::Invalid(_) => "CFG-INVALID",
        fenrir::config::ConfigError::InvalidMessage(_) => "CFG-INVALID",
        fenrir::config::ConfigError::MissingConfigFile { .. } => "CFG-MISSING-FILE",
        fenrir::config::ConfigError::InvalidProfile { .. } => "CFG-INVALID-PROFILE",
        fenrir::config::ConfigError::Anyhow(_) => "CFG-DESERIALIZE",
    }
}
