use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::ErrorKind;
use tracing::{info, warn};

use crate::domain::db::{DbEngine, DbError, DbExecutionResult};
use crate::services::db_shell::DbShellService;

use super::{list_migration_files, migration_dir};

#[derive(Debug, Default, Serialize, Deserialize)]
struct MigrationState {
    applied: BTreeSet<String>,
    /// Hash of the database identity (to detect if DB was recreated)
    #[serde(default)]
    db_fingerprint: Option<String>,
}

impl MigrationState {
    fn mark_applied(&mut self, file: String) {
        self.applied.insert(file);
    }

    fn is_applied(&self, file: &str) -> bool {
        self.applied.contains(file)
    }

    fn clear(&mut self) {
        self.applied.clear();
    }

    fn set_fingerprint(&mut self, fingerprint: String) {
        self.db_fingerprint = Some(fingerprint);
    }
}

#[derive(Debug, Default)]
pub struct MigrationReport {
    applied: Vec<String>,
    /// True if migration state was reset because DB was recreated
    pub db_was_recreated: bool,
}

impl MigrationReport {
    pub fn applied(&self) -> &[String] {
        &self.applied
    }

    pub fn applied_count(&self) -> usize {
        self.applied.len()
    }

    fn record(&mut self, file: String) {
        self.applied.push(file);
    }
}

pub async fn apply_pending_migrations(
    runtime_dir: PathBuf,
    db_shell: Arc<DbShellService>,
    engine: DbEngine,
) -> Result<MigrationReport> {
    let migrations = list_migration_files(engine.as_str())?;
    if migrations.is_empty() {
        return Ok(MigrationReport::default());
    }

    let mut session = db_shell.create_session();
    session
        .switch_engine(engine)
        .map_err(|err| anyhow!(err.to_string()))?;

    let state_path = state_file(&runtime_dir, engine);
    let mut state = load_state(&state_path).await?;
    let mut report = MigrationReport::default();

    // Check if database was recreated by comparing fingerprints
    let current_fingerprint = get_db_fingerprint(&session, engine).await;
    if let Some(ref stored_fp) = state.db_fingerprint {
        if let Some(ref current_fp) = current_fingerprint {
            if stored_fp != current_fp {
                warn!(
                    engine = %engine,
                    "database fingerprint changed - migrations will be re-applied"
                );
                state.clear();
                report.db_was_recreated = true;
            }
        }
    }

    // Also check if a known table exists (fallback if fingerprint fails)
    if !state.applied.is_empty() && !report.db_was_recreated {
        let tables_exist = check_migrations_applied(&session, engine).await;
        if !tables_exist {
            warn!(
                engine = %engine,
                "migration tables not found in database - resetting migration state"
            );
            state.clear();
            report.db_was_recreated = true;
        }
    }

    for file in migrations {
        if state.is_applied(&file) {
            continue;
        }
        let full_path = migration_dir(engine.as_str()).join(&file);
        if !full_path.exists() {
            continue;
        }
        let payload = fs::read_to_string(&full_path)
            .await
            .with_context(|| format!("failed to read migration {}", full_path.display()))?;
        if payload.trim().is_empty() {
            state.mark_applied(file.clone());
            persist_state(&state_path, &state).await?;
            continue;
        }
        info!(
            engine = %engine,
            file = %file,
            "applying database migration"
        );
        session
            .simple_query(payload.as_str())
            .await
            .map_err(db_error)?;
        state.mark_applied(file.clone());
        persist_state(&state_path, &state).await?;
        report.record(file);
    }

    // Update fingerprint after successful migrations
    if let Some(fp) = current_fingerprint {
        state.set_fingerprint(fp);
        persist_state(&state_path, &state).await?;
    }

    Ok(report)
}

/// Get a database fingerprint to detect if DB was recreated
async fn get_db_fingerprint(
    session: &crate::services::db_shell::DbShellSession,
    engine: DbEngine,
) -> Option<String> {
    let query = match engine {
        DbEngine::Postgres => {
            // Get database OID which changes when DB is recreated
            "SELECT oid::text FROM pg_database WHERE datname = current_database()"
        }
        DbEngine::Sqlite => {
            // SQLite doesn't have a reliable fingerprint, skip
            return None;
        }
        _ => return None,
    };

    match session.simple_query(query).await {
        Ok(results) => {
            if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
                if let Some(row) = rs.rows.first() {
                    if let Some(oid) = row.first() {
                        return Some(oid.clone());
                    }
                }
            }
            None
        }
        Err(_) => None,
    }
}

/// Check if migration tables actually exist in the database
async fn check_migrations_applied(
    session: &crate::services::db_shell::DbShellSession,
    engine: DbEngine,
) -> bool {
    // Check for a table that should exist if migrations ran
    // Using identity_keys as it's from our identity migration
    let query = match engine {
        DbEngine::Postgres => {
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'identity_keys')"
        }
        DbEngine::Sqlite => {
            "SELECT name FROM sqlite_master WHERE type='table' AND name='identity_keys'"
        }
        _ => return true, // Assume tables exist for unsupported engines
    };

    match session.simple_query(query).await {
        Ok(results) => {
            if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
                if let Some(row) = rs.rows.first() {
                    if let Some(val) = row.first() {
                        // Postgres returns "t" for true, SQLite returns the table name
                        return val == "t" || val == "true" || val == "identity_keys";
                    }
                }
            }
            false
        }
        Err(_) => false,
    }
}

async fn load_state(path: &Path) -> Result<MigrationState> {
    match fs::read_to_string(path).await {
        Ok(raw) => {
            let state = serde_json::from_str(&raw)
                .with_context(|| format!("failed to parse migration state {}", path.display()))?;
            Ok(state)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(MigrationState::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

async fn persist_state(path: &Path, state: &MigrationState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let payload = serde_json::to_vec_pretty(state)?;
    fs::write(path, payload)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn state_file(runtime_dir: &Path, engine: DbEngine) -> PathBuf {
    runtime_dir
        .join("migrations")
        .join(format!("{}.json", engine.as_str()))
}

fn db_error(err: DbError) -> anyhow::Error {
    anyhow!(err.to_string())
}
