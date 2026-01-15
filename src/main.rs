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

    /// Run database migrations and exit
    #[arg(long)]
    migrate: bool,

    /// Check if password is set for a user and exit (0=set, 1=not set)
    #[arg(long, value_name = "USER_ID")]
    password_status: Option<String>,

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

fn main() {
    let args = Args::parse_from(normalize_args());

    // Handle quick-exit commands before starting tokio runtime
    if args.check_config {
        handle_check_config();
    }

    if args.migrate {
        handle_migrate();
    }

    if let Some(ref user_id) = args.password_status {
        handle_password_status(user_id);
    }

    // Start the async runtime for the main application
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
        .block_on(async_main(args));
}

async fn async_main(args: Args) {
    if let Some(config_path) = args.dev_agent_config {
        if let Err(err) = fenrir::dev_agent::run(config_path).await {
            eprintln!("dev agent failed: {err}");
            std::process::exit(1);
        }
        return;
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

fn handle_migrate() {
    use fenrir::config::{DbRuntimeMode, EmbeddedEngineKind};
    use fenrir::domain::db::DbEngine;
    use fenrir::infra::db;
    use fenrir::services::db_shell::DbShellService;
    use fenrir::services::ManagedService;
    use tokio::runtime::Runtime;

    let cfg = match fenrir::config::load() {
        Ok(c) => Arc::new(c),
        Err(err) => {
            eprintln!("CFG-ERROR: {err}");
            std::process::exit(2);
        }
    };

    let rt = Runtime::new().expect("Failed to create tokio runtime");
    // Use config-based runtime paths
    let runtime_root = cfg.runtime.resolve_base_path();
    let db_dir = cfg.runtime.db_path();

    // Start DB runtime if embedded
    let db_runtime = if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
        println!("MIGRATE: starting embedded database...");
        let embedded = &cfg.db.runtime.embedded;
        let supervisor = db::runtime::DbRuntimeSupervisor::new_early(
            cfg.db.runtime.mode,
            embedded.engine,
            embedded.postgres.port_range,
            embedded.postgres.binary_path.clone(),
            db_dir,
            embedded.security.clone(),
        );
        
        // Attach early SecurityManager if password auth is required
        if embedded.security.auth_method.requires_password() {
            use fenrir::security::manager::{NoopAuditSink, SecurityManager};
            let noop_audit = std::sync::Arc::new(NoopAuditSink);
            match SecurityManager::new(&cfg.security, noop_audit) {
                Ok(security) => {
                    supervisor.attach_security(std::sync::Arc::new(security));
                }
                Err(e) => {
                    eprintln!("MIGRATE-SECURITY: failed to create security manager ({e})");
                    std::process::exit(1);
                }
            }
        }
        
        match rt.block_on(supervisor.clone().start()) {
            Ok(_) => Some(supervisor),
            Err(e) => {
                eprintln!("MIGRATE-DB-START: failed to start database ({e})");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // Get connector URI for adapters and build override
    let embedded_engine = cfg.db.runtime.embedded.engine;
    let runtime_override = db_runtime.as_ref().and_then(|r| {
        r.connector_uri().map(|uri| {
            let engine = match embedded_engine {
                EmbeddedEngineKind::Sqlite => DbEngine::Sqlite,
                EmbeddedEngineKind::Postgres => DbEngine::Postgres,
            };
            db::manager::RuntimeDbOverride { engine, uri }
        })
    });

    // Create DB adapters
    let adapters = match db::manager::build_adapters(&cfg, runtime_override) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("MIGRATE-DB-ADAPTERS: {e}");
            std::process::exit(1);
        }
    };

    // Determine effective engine
    let effective_engine: DbEngine = if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
        match cfg.db.runtime.embedded.engine {
            EmbeddedEngineKind::Sqlite => DbEngine::Sqlite,
            EmbeddedEngineKind::Postgres => DbEngine::Postgres,
        }
    } else {
        match cfg.db.default_engine.parse::<DbEngine>() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("MIGRATE-DB-ENGINE: invalid default engine ({e})");
                std::process::exit(1);
            }
        }
    };

    // Create DB shell service
    let db_shell = match DbShellService::new(effective_engine, adapters) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("MIGRATE-DB-SHELL: failed to create db shell ({e})");
            std::process::exit(1);
        }
    };

    // Run migrations
    println!("MIGRATE: applying pending migrations...");
    match rt.block_on(db::migrations::apply_pending_migrations(
        runtime_root,
        db_shell,
        effective_engine,
    )) {
        Ok(report) => {
            let applied = report.applied();
            if applied.is_empty() {
                println!("MIGRATE-OK: database already up to date");
            } else {
                println!(
                    "MIGRATE-OK: applied {} migration(s): {}",
                    applied.len(),
                    applied.join(", ")
                );
            }
        }
        Err(e) => {
            eprintln!("MIGRATE-ERROR: {e}");
            std::process::exit(1);
        }
    }

    // Stop DB runtime if we started it
    if let Some(supervisor) = db_runtime {
        let _ = rt.block_on(supervisor.stop(false));
    }

    std::process::exit(0);
}

fn handle_password_status(user_id: &str) {
    use fenrir::config::IdentityStorageKind;

    let cfg = match fenrir::config::load() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("CFG-ERROR: {err}");
            std::process::exit(2);
        }
    };

    // Use config-based runtime paths
    let identity_dir = cfg.runtime.identity_path();

    let storage_kind = cfg.security.identity.embedded.storage;

    // Check for pending password file first (works for both modes)
    let pending_path = identity_dir.join(".pending-password");
    if pending_path.exists() {
        // Pending password exists - will be set on first boot
        println!("PWD-PENDING password pending for '{}'", user_id);
        std::process::exit(0);
    }

    match storage_kind {
        IdentityStorageKind::File => {
            // Check JSON file
            let store_path = identity_dir.join("store.json");
            if !store_path.exists() {
                println!("PWD-NOT-SET no password for '{}'", user_id);
                std::process::exit(1);
            }
            match std::fs::read_to_string(&store_path) {
                Ok(content) => {
                    if content.contains("password_hash") {
                        println!("PWD-SET password configured for '{}'", user_id);
                        std::process::exit(0);
                    } else {
                        println!("PWD-NOT-SET no password for '{}'", user_id);
                        std::process::exit(1);
                    }
                }
                Err(_) => {
                    println!("PWD-NOT-SET no password for '{}'", user_id);
                    std::process::exit(1);
                }
            }
        }
        IdentityStorageKind::Db => {
            // For DB mode, we need to check the database
            // This is a lightweight check - we just verify the identity_users table has a password
            // We can't do a full DB query without starting the DB adapter,
            // so we use a marker file approach
            let db_marker = identity_dir.join(".db-password-set");
            if db_marker.exists() {
                println!("PWD-SET password configured for '{}' (db)", user_id);
                std::process::exit(0);
            } else {
                println!("PWD-NOT-SET no password for '{}' (db)", user_id);
                std::process::exit(1);
            }
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
