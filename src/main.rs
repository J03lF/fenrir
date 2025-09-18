use clap::Parser;

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
    println!(
        "{} v{} started",
        ctx.config.app.name, ctx.config.app.version
    );
    if let Err(e) = fenrir::boot::start_transports(&ctx).await {
        eprintln!("transport init failed: {e}");
    }

    if args.cli {
        if let Err(e) = fenrir::cli::shell::run_shell(&ctx.config) {
            eprintln!("cli error: {e}");
        }
    } else {
        // Keep running if started without CLI to serve SSH
        let _ = tokio::signal::ctrl_c().await;
    }
}
