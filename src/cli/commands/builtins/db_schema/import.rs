use std::path::PathBuf;
use std::time::Instant;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::domain::db::DbEngine;
use crate::infra::db::migrations::planner::{plan_to_sql, PlanOptions};
use crate::services::db_schema::{diff, staruml_parser, DatabaseBlueprint};
use crate::services::AppServices;

/// Progress updates during schema import
#[derive(Debug, Clone)]
pub enum ImportProgress {
    /// Reading schema file
    ReadingFile,
    /// Parsing StarUML schema
    ParsingSchema,
    /// Creating database snapshot
    SnapshotDatabase,
    /// Comparing schemas
    ComparingSchemas,
    /// Planning migrations
    PlanningMigrations,
    /// Applying statement (current, total)
    ApplyingStatement(usize, usize),
    /// Import complete
    Complete,
}

/// Import result with details
#[derive(Debug, Clone)]
pub struct ImportResult {
    pub message: String,
    pub statements_count: usize,
    pub dry_run: bool,
}

pub async fn run_import(
    services: &AppServices,
    path: PathBuf,
    engine: Option<DbEngine>,
    dry_run: bool,
    force: bool,
) -> Result<String, String> {
    run_import_with_progress(services, path, engine, dry_run, force, None::<fn(ImportProgress)>)
        .await
        .map(|r| r.message)
}

pub async fn run_import_with_progress<F>(
    services: &AppServices,
    path: PathBuf,
    engine: Option<DbEngine>,
    dry_run: bool,
    force: bool,
    progress: Option<F>,
) -> Result<ImportResult, String>
where
    F: Fn(ImportProgress) + Send + Sync,
{
    let report = |p: ImportProgress| {
        if let Some(ref cb) = progress {
            cb(p);
        }
    };

    let started = Instant::now();

    report(ImportProgress::ReadingFile);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(err) => return Err(err.to_string()),
    };

    report(ImportProgress::ParsingSchema);
    let current_engine = services.db_shell.default_engine();
    let desired_engine = engine.unwrap_or(current_engine);
    let desired = match staruml_parser::parse_staruml(&bytes, desired_engine) {
        Ok(b) => b,
        Err(err) => return Err(err.to_string()),
    };

    report(ImportProgress::SnapshotDatabase);
    let current =
        match DatabaseBlueprint::snapshot(services.db_shell.clone(), Some(desired_engine)).await {
            Ok(b) => b,
            Err(err) => return Err(err.to_string()),
        };

    report(ImportProgress::ComparingSchemas);
    let plan = diff::diff(&desired, &current);
    if plan.operations.is_empty() {
        report(ImportProgress::Complete);
        return Ok(ImportResult {
            message: "no changes detected".to_string(),
            statements_count: 0,
            dry_run,
        });
    }

    report(ImportProgress::PlanningMigrations);
    let opts = PlanOptions { force, dry_run };
    let planned = match plan_to_sql(&plan, &opts) {
        Ok(p) => p,
        Err(err) => return Err(err.to_string()),
    };

    if dry_run {
        let path_info = planned.path.as_deref().unwrap_or("<not saved>");
        record_probe(services, started.elapsed().as_secs_f64() * 1000.0, true);
        record_audit(services, desired_engine, plan.operations.len(), true);
        report(ImportProgress::Complete);
        return Ok(ImportResult {
            message: format!(
                "dry-run: {} statements, saved to {}",
                planned.statements.len(),
                path_info
            ),
            statements_count: planned.statements.len(),
            dry_run,
        });
    }

    let mut session = services.db_shell.create_session();
    if let Err(err) = session.switch_engine(desired_engine) {
        return Err(err.to_string());
    }

    let total = planned.statements.len();
    for (idx, stmt) in planned.statements.iter().enumerate() {
        report(ImportProgress::ApplyingStatement(idx + 1, total));
        if let Err(err) = session.simple_query(stmt).await {
            record_probe(services, started.elapsed().as_secs_f64() * 1000.0, false);
            record_audit(services, desired_engine, plan.operations.len(), false);
            return Err(format!("failed on `{stmt}`: {err}"));
        }
    }

    record_probe(services, started.elapsed().as_secs_f64() * 1000.0, true);
    record_audit(services, desired_engine, plan.operations.len(), true);
    report(ImportProgress::Complete);

    Ok(ImportResult {
        message: format!(
            "applied {} statements to {}",
            planned.statements.len(),
            desired_engine.as_str()
        ),
        statements_count: planned.statements.len(),
        dry_run,
    })
}

fn record_probe(services: &AppServices, latency_ms: f64, success: bool) {
    services
        .diagnostics()
        .record_probe("db-schema-import", latency_ms, success);
    services.diagnostics().record_heartbeat("db-schema-import");
}

fn record_audit(services: &AppServices, engine: DbEngine, ops: usize, success: bool) {
    if let Ok(event) = AuditEvent::builder()
        .actor(AuditActor::System)
        .action("db::schema::import")
        .outcome(if success {
            AuditOutcome::Success
        } else {
            AuditOutcome::Failure
        })
        .metadata(
            AuditMetadata::default()
                .insert("engine", engine.as_str())
                .insert("operations", ops.to_string()),
        )
        .build()
    {
        let _ = services.record_audit(event);
    }
}
