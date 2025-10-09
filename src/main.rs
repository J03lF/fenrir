use clap::Parser;
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
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if args.check_config {
        match fenrir::config::load() {
            Ok(_) => {
                println!("config OK");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("config ERROR: {err}");
                std::process::exit(1);
            }
        }
    }

    // Boot sequence (logging/telemetry)
    let ctx = match fenrir::boot::boot() {
        Ok(ctx) => ctx,
        Err(err) => {
            eprintln!("boot failed: {err}");
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
