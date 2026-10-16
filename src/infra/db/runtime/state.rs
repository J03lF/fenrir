use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::fs;

use crate::config::EmbeddedEngineKind;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AdapterHealthStatus {
    #[serde(rename = "unknown")]
    #[default]
    Unknown,
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "degraded")]
    Degraded,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "stopped")]
    Stopped,
}

impl AdapterHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterHealthStatus::Unknown => "unknown",
            AdapterHealthStatus::Ready => "ready",
            AdapterHealthStatus::Degraded => "degraded",
            AdapterHealthStatus::Failed => "failed",
            AdapterHealthStatus::Stopped => "stopped",
        }
    }

    pub const fn as_code(self) -> u64 {
        match self {
            AdapterHealthStatus::Unknown => 0,
            AdapterHealthStatus::Ready => 1,
            AdapterHealthStatus::Degraded => 2,
            AdapterHealthStatus::Failed => 3,
            AdapterHealthStatus::Stopped => 4,
        }
    }

    pub const fn variants() -> [Self; 5] {
        [
            AdapterHealthStatus::Unknown,
            AdapterHealthStatus::Ready,
            AdapterHealthStatus::Degraded,
            AdapterHealthStatus::Failed,
            AdapterHealthStatus::Stopped,
        ]
    }

    pub const fn metric_key(self) -> &'static str {
        match self {
            AdapterHealthStatus::Unknown => "db.runtime.adapter.status.unknown",
            AdapterHealthStatus::Ready => "db.runtime.adapter.status.ready",
            AdapterHealthStatus::Degraded => "db.runtime.adapter.status.degraded",
            AdapterHealthStatus::Failed => "db.runtime.adapter.status.failed",
            AdapterHealthStatus::Stopped => "db.runtime.adapter.status.stopped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdapterHealthState {
    #[serde(default)]
    pub status: AdapterHealthStatus,
    #[serde(default)]
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_health: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub connector_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRuntimeState {
    pub engine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checkpoint: Option<OffsetDateTime>,
    #[serde(default)]
    pub applied_migrations: Vec<String>,
    #[serde(default)]
    pub adapter_health: AdapterHealthState,
    pub updated_at: OffsetDateTime,
}

impl DbRuntimeState {
    pub fn new(
        engine: EmbeddedEngineKind,
        last_checkpoint: Option<OffsetDateTime>,
        applied_migrations: Vec<String>,
        adapter_health: AdapterHealthState,
    ) -> Self {
        Self {
            engine: engine.as_str().to_string(),
            last_checkpoint,
            applied_migrations,
            adapter_health,
            updated_at: OffsetDateTime::now_utc(),
        }
    }
}

pub async fn collect_snapshot_with_health(
    runtime_root: &Path,
    engine: EmbeddedEngineKind,
    last_checkpoint: Option<OffsetDateTime>,
    adapter_health: AdapterHealthState,
) -> anyhow::Result<DbRuntimeState> {
    let applied = read_applied_migrations(runtime_root, engine).await?;
    Ok(DbRuntimeState::new(
        engine,
        last_checkpoint,
        applied,
        adapter_health,
    ))
}

pub async fn sync_from_migrations(
    runtime_root: &Path,
    engine: EmbeddedEngineKind,
) -> anyhow::Result<()> {
    let adapter_health = load_snapshot(runtime_root)
        .await?
        .map(|snapshot| snapshot.adapter_health)
        .unwrap_or_default();
    let snapshot = collect_snapshot_with_health(runtime_root, engine, None, adapter_health).await?;
    persist_snapshot(runtime_root, &snapshot).await
}

pub async fn persist_snapshot(
    runtime_root: &Path,
    snapshot: &DbRuntimeState,
) -> anyhow::Result<()> {
    let path = state_file(runtime_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create runtime dir {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(snapshot)?;
    fs::write(&path, payload)
        .await
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub async fn load_snapshot(runtime_root: &Path) -> anyhow::Result<Option<DbRuntimeState>> {
    let path = state_file(runtime_root);
    match fs::read_to_string(&path).await {
        Ok(raw) => {
            let snapshot = serde_json::from_str::<DbRuntimeState>(&raw)
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(Some(snapshot))
        }
        Err(err) => {
            if err.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(err).with_context(|| format!("read {}", path.display()))
            }
        }
    }
}

pub async fn read_applied_migrations(
    runtime_root: &Path,
    engine: EmbeddedEngineKind,
) -> anyhow::Result<Vec<String>> {
    let state_path = migration_state_file(runtime_root, engine);
    let raw = match fs::read_to_string(&state_path).await {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("read {}", state_path.display()));
        }
    };
    let state: MigrationSummary =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", state_path.display()))?;
    let mut applied = state.applied.unwrap_or_default();
    applied.sort();
    Ok(applied)
}

pub fn state_file(runtime_root: &Path) -> PathBuf {
    runtime_root.join("db_state.json")
}

fn migration_state_file(runtime_root: &Path, engine: EmbeddedEngineKind) -> PathBuf {
    runtime_root
        .join("migrations")
        .join(format!("{}.json", engine.as_str()))
}

#[derive(Debug, Deserialize)]
struct MigrationSummary {
    #[serde(default)]
    applied: Option<Vec<String>>,
}
