use std::env;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(name = "fenrirctl", about = "Fenrir control plane client")]
struct Cli {
    /// Base URL of the Fenrir HTTP control plane
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    url: String,

    /// Explicit control-plane token (overrides env vars)
    #[arg(long)]
    token: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Release all active dev overrides
    ReleaseDevOverrides,
    /// Stop all running modules
    StopModules,
    /// Release overrides and stop modules sequentially
    Shutdown,
    /// Show installed modules
    Status,
    /// Show db-runtime status (embedded mode)
    DbRuntimeStatus {
        /// Tail N log lines
        #[arg(long, default_value = "0")]
        tail: usize,
    },
    /// Show db-runtime logs
    DbRuntimeLogs {
        /// Tail N log lines
        #[arg(long, default_value = "50")]
        tail: usize,
    },
    /// Start db-runtime
    DbRuntimeStart,
    /// Stop db-runtime
    DbRuntimeStop,
    /// Restart db-runtime
    DbRuntimeRestart,
}

#[derive(Deserialize, Debug)]
struct InstalledModulesResponse {
    modules: Vec<InstalledModuleEntry>,
}

#[derive(Deserialize, Debug)]
struct InstalledModuleEntry {
    manifest: ModuleManifest,
    installed_at: Option<String>,
    path: String,
    source: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct ModuleManifest {
    id: String,
    version: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let token = resolve_token(cli.token)?;
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    match cli.command {
        Command::ReleaseDevOverrides => {
            release_dev_overrides(&client, &cli.url, &token).await?;
            println!("released dev overrides");
        }
        Command::StopModules => {
            stop_modules(&client, &cli.url, &token).await?;
            println!("module runtime stop-all request acknowledged");
        }
        Command::Shutdown => {
            release_dev_overrides(&client, &cli.url, &token).await?;
            stop_modules(&client, &cli.url, &token).await?;
            stop_services(&client, &cli.url, &token).await?;
            println!("shutdown sequence completed");
        }
        Command::Status => {
            show_status(&client, &cli.url, &token).await?;
        }
        Command::DbRuntimeStatus { tail } => {
            db_runtime_status(&client, &cli.url, &token, tail).await?;
        }
        Command::DbRuntimeLogs { tail } => {
            db_runtime_logs(&client, &cli.url, &token, tail).await?;
        }
        Command::DbRuntimeStart => {
            control_db_runtime(&client, &cli.url, &token, "start").await?;
            println!("db-runtime start requested");
        }
        Command::DbRuntimeStop => {
            control_db_runtime(&client, &cli.url, &token, "stop").await?;
            println!("db-runtime stop requested");
        }
        Command::DbRuntimeRestart => {
            control_db_runtime(&client, &cli.url, &token, "restart").await?;
            println!("db-runtime restart requested");
        }
    }

    Ok(())
}

fn resolve_token(explicit: Option<String>) -> Result<String> {
    if let Some(token) = explicit {
        return ensure_token(token);
    }
    if let Ok(token) = env::var("FENRIR_CONTROL_TOKEN") {
        return ensure_token(token);
    }
    if let Ok(token) = env::var("FENRIR_HTTP_TOKEN_ADMIN") {
        return ensure_token(token);
    }
    Err(anyhow!(
        "control token missing – set FENRIR_CONTROL_TOKEN or pass --token"
    ))
}

fn ensure_token(token: String) -> Result<String> {
    if token.trim().is_empty() {
        Err(anyhow!("control token must not be empty"))
    } else {
        Ok(token)
    }
}

async fn release_dev_overrides(client: &Client, base_url: &str, token: &str) -> Result<()> {
    post_empty(
        client,
        format!("{base_url}/modules/runtime/release-dev-overrides"),
        token,
    )
    .await
}

async fn stop_modules(client: &Client, base_url: &str, token: &str) -> Result<()> {
    let url = format!("{base_url}/modules/runtime/stop-all");
    let resp = client
        .post(url.clone())
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    if resp.status().is_success() {
        return Ok(());
    }
    if resp.status() == StatusCode::NOT_FOUND {
        eprintln!(
            "[Task] module runtime stop-all endpoint missing – falling back to sequential shutdown"
        );
        return stop_modules_fallback(client, base_url, token).await;
    }
    let body = resp.text().await.unwrap_or_default();
    Err(anyhow!("request to {url} failed: {body}"))
}

async fn stop_services(client: &Client, base_url: &str, token: &str) -> Result<()> {
    post_empty(
        client,
        format!("{base_url}/services/actions/stop-all"),
        token,
    )
    .await
}

async fn db_runtime_status(client: &Client, base_url: &str, token: &str, tail: usize) -> Result<()> {
    let url = format!("{base_url}/services/db-runtime/status?tail={tail}");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("request to {url} failed: {body}"));
    }
    let body = resp.text().await?;
    println!("{body}");
    Ok(())
}

async fn db_runtime_logs(client: &Client, base_url: &str, token: &str, tail: usize) -> Result<()> {
    let url = format!("{base_url}/services/db-runtime/logs?tail={tail}");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("request to {url} failed: {body}"));
    }
    let body = resp.text().await?;
    println!("{body}");
    Ok(())
}

async fn control_db_runtime(
    client: &Client,
    base_url: &str,
    token: &str,
    action: &str,
) -> Result<()> {
    let url = format!("{base_url}/services/db-runtime/{action}");
    post_empty(client, url, token).await
}

async fn show_status(client: &Client, base_url: &str, token: &str) -> Result<()> {
    let modules = fetch_installed_modules(client, base_url, token).await?;
    if modules.is_empty() {
        println!("no modules installed");
        return Ok(());
    }
    println!("installed modules:");
    for module in modules {
        let installed = module
            .installed_at
            .as_deref()
            .unwrap_or("unknown timestamp");
        println!(
            " - {} v{} (installed_at: {installed}, path: {}, source: {})",
            module.manifest.id, module.manifest.version, module.path, module.source,
        );
    }
    Ok(())
}

async fn fetch_installed_modules(
    client: &Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<InstalledModuleEntry>> {
    let resp = client
        .get(format!("{base_url}/modules/installed"))
        .bearer_auth(token)
        .send()
        .await
        .context("status request failed")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("status request failed: {}", body));
    }
    let payload: InstalledModulesResponse = resp.json().await?;
    Ok(payload.modules)
}

async fn post_empty(client: &Client, url: String, token: &str) -> Result<()> {
    let resp = client
        .post(url.clone())
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(anyhow!("request to {url} failed: {body}"))
    }
}

async fn stop_modules_fallback(client: &Client, base_url: &str, token: &str) -> Result<()> {
    let modules = fetch_installed_modules(client, base_url, token).await?;
    if modules.is_empty() {
        return Ok(());
    }
    for module in modules {
        let module_id = module.manifest.id;
        let url = format!("{base_url}/modules/runtime/{module_id}/stop");
        let resp = client
            .post(url.clone())
            .bearer_auth(token)
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        if resp.status().is_success() || resp.status() == StatusCode::CONFLICT {
            continue;
        }
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("request to {url} failed: {body}"));
    }
    Ok(())
}
