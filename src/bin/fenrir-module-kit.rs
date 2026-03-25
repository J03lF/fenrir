use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const MODULE_ID_ENV: &str = "FENRIR_MODULE_ID";
const SERVICE_ID_ENV: &str = "FENRIR_SERVICE_ID";
const SERVICE_URI_ENV: &str = "FENRIR_SERVICE_URI";
const SERVICE_TOKEN_ENV: &str = "FENRIR_SERVICE_TOKEN";
const TOKEN_ISSUED_AT_ENV: &str = "FENRIR_SERVICE_TOKEN_ISSUED_AT";
const TOKEN_EXPIRES_AT_ENV: &str = "FENRIR_SERVICE_TOKEN_EXPIRES_AT";
const TOKEN_TTL_ENV: &str = "FENRIR_SERVICE_TOKEN_TTL_SECS";
const SNAPSHOT_ENV: &str = "FENRIR_SERVICE_SNAPSHOT_PATH";
const DB_RUNTIME_MODE_ENV: &str = "FENRIR_DB_RUNTIME_MODE";
const DB_RUNTIME_URI_ENV: &str = "FENRIR_DB_RUNTIME_URI";

#[derive(Parser)]
#[command(name = "fenrir-module-kit")]
#[command(about = "Utility helpers for Fenrir modules")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Prepare runtime metadata for a module before the business binary starts
    Init(InitArgs),
}

#[derive(Args)]
struct InitArgs {
    /// Path to the module root directory
    #[arg(long, value_name = "PATH")]
    module_root: PathBuf,
    /// Optional override for the runtime metadata path
    #[arg(long, value_name = "PATH")]
    runtime_file: Option<PathBuf>,
    /// Optional override for the service snapshot path (defaults to FENRIR_SERVICE_SNAPSHOT_PATH)
    #[arg(long, value_name = "PATH")]
    snapshot: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = cli.run() {
        eprintln!("fenrir-module-kit init failed: {err}");
        std::process::exit(1);
    }
}

impl Cli {
    fn run(self) -> Result<()> {
        match self.command {
            Command::Init(args) => run_init(args),
        }
    }
}

fn run_init(args: InitArgs) -> Result<()> {
    validate_module_root(&args.module_root)?;
    let output_path = resolve_runtime_path(&args.module_root, args.runtime_file);
    let snapshot_entries = read_snapshot_entries(args.snapshot.as_deref())?;

    let module_id = required_env(MODULE_ID_ENV)?;
    let service_id = required_env(SERVICE_ID_ENV)?;
    let service_uri = optional_env(SERVICE_URI_ENV);
    let token = required_env(SERVICE_TOKEN_ENV)?;
    let issued_at = optional_env(TOKEN_ISSUED_AT_ENV);
    let expires_at = optional_env(TOKEN_EXPIRES_AT_ENV);
    let ttl_secs = optional_u64_env(TOKEN_TTL_ENV)?;

    let runtime = ModuleRuntimeMetadata {
        module: ModuleDescriptor {
            id: module_id,
            service_id,
            service_uri,
        },
        token: TokenDescriptor {
            value: token,
            issued_at,
            expires_at,
            ttl_secs,
        },
        service_snapshot: ServiceSnapshotSection {
            path: service_snapshot_path(args.snapshot.as_deref()),
            services: snapshot_entries,
        },
        db_runtime: read_db_runtime_env()?,
        generated_at: current_timestamp(),
    };

    write_runtime_file(&output_path, &runtime)
}

fn read_snapshot_entries(override_path: Option<&Path>) -> Result<Vec<ServiceSnapshotEntry>> {
    let path = match override_path {
        Some(path) => Some(path.to_path_buf()),
        None => optional_env(SNAPSHOT_ENV).map(PathBuf::from),
    };
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Err(anyhow!(
            "service snapshot file {} does not exist",
            path.display()
        ));
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read service snapshot {}", path.display()))?;
    let parsed: RawServiceSnapshot = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse service snapshot JSON at {}",
            path.display()
        )
    })?;
    Ok(parsed.services)
}

fn service_snapshot_path(override_path: Option<&Path>) -> Option<String> {
    override_path
        .map(|path| path.to_path_buf())
        .or_else(|| optional_env(SNAPSHOT_ENV).map(PathBuf::from))
        .map(|path| path.to_string_lossy().to_string())
}

fn read_db_runtime_env() -> Result<Option<DbRuntimeSection>> {
    let mode = optional_env(DB_RUNTIME_MODE_ENV);
    let uri = optional_env(DB_RUNTIME_URI_ENV);
    if mode.is_none() && uri.is_none() {
        return Ok(None);
    }
    Ok(Some(DbRuntimeSection { mode, uri }))
}

fn write_runtime_file(path: &Path, data: &ModuleRuntimeMetadata) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to prepare runtime directory {}", parent.display()))?;
    }
    let payload =
        serde_json::to_vec_pretty(data).context("failed to serialize runtime metadata")?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open runtime metadata {}", path.display()))?;
    file.write_all(&payload)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    Ok(())
}

fn validate_module_root(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("module root {} does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(anyhow!("module root {} is not a directory", path.display()));
    }
    Ok(())
}

fn resolve_runtime_path(root: &Path, override_path: Option<PathBuf>) -> PathBuf {
    match override_path {
        Some(path) => path,
        None => root.join(".fenrir").join("runtime.json"),
    }
}

fn required_env(name: &str) -> Result<String> {
    optional_env(name).ok_or_else(|| anyhow!("{name} is not set"))
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn optional_u64_env(name: &str) -> Result<Option<u64>> {
    match optional_env(name) {
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|err| anyhow!("failed to parse {name} as integer (value: {value}): {err}")),
        None => Ok(None),
    }
}

fn current_timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&Rfc3339)
        .unwrap_or_else(|_| now.unix_timestamp().to_string())
}

#[derive(Serialize)]
struct ModuleRuntimeMetadata {
    module: ModuleDescriptor,
    token: TokenDescriptor,
    service_snapshot: ServiceSnapshotSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    db_runtime: Option<DbRuntimeSection>,
    generated_at: String,
}

#[derive(Serialize)]
struct ModuleDescriptor {
    id: String,
    service_id: String,
    service_uri: Option<String>,
}

#[derive(Serialize)]
struct TokenDescriptor {
    value: String,
    issued_at: Option<String>,
    expires_at: Option<String>,
    ttl_secs: Option<u64>,
}

#[derive(Serialize)]
struct ServiceSnapshotSection {
    path: Option<String>,
    services: Vec<ServiceSnapshotEntry>,
}

#[derive(Serialize)]
struct DbRuntimeSection {
    mode: Option<String>,
    uri: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ServiceSnapshotEntry {
    uri: String,
    endpoint: String,
    #[serde(default)]
    endpoints: Vec<String>,
}

#[derive(Deserialize)]
struct RawServiceSnapshot {
    services: Vec<ServiceSnapshotEntry>,
}
