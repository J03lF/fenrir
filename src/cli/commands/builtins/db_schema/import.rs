use std::path::PathBuf;
use std::time::Instant;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::domain::db::DbEngine;
use crate::infra::db::migrations::planner::{plan_to_sql, PlanOptions};
use crate::services::AppServices;
use crate::services::db_schema::{diff, staruml_parser, DatabaseBlueprint};

pub async fn run_import(
    services: &AppServices,
    path: PathBuf,
    engine: Option<DbEngine>,
    dry_run: bool,
    force: bool,
) -> Result<String, String> {
    let started = Instant::now();
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(err) => return Err(err.to_string()),
    };

    let current_engine = services.db_shell.default_engine();
    let desired_engine = engine.unwrap_or(current_engine);
    let desired = match staruml_parser::parse_staruml(&bytes, desired_engine) {
        Ok(b) => b,
        Err(err) => return Err(err.to_string()),
    };

    let current =
        match DatabaseBlueprint::snapshot(services.db_shell.clone(), Some(desired_engine)).await {
            Ok(b) => b,
            Err(err) => return Err(err.to_string()),
        };

    let plan = diff::diff(&desired, &current);
    if plan.operations.is_empty() {
        return Ok("no changes detected".to_string());
    }

    let opts = PlanOptions { force, dry_run };
    let planned = match plan_to_sql(&plan, &opts) {
        Ok(p) => p,
        Err(err) => return Err(err.to_string()),
    };

    if dry_run {
        let path_info = planned.path.as_deref().unwrap_or("<not saved>");
        record_probe(services, started.elapsed().as_secs_f64() * 1000.0, true);
        record_audit(services, desired_engine, plan.operations.len(), true);
        return Ok(format!(
            "dry-run: {} statements, saved to {}",
            planned.statements.len(),
            path_info
        ));
    }

    let mut session = services.db_shell.create_session();
    if let Err(err) = session.switch_engine(desired_engine) {
        return Err(err.to_string());
    }
    for stmt in &planned.statements {
        if let Err(err) = session.simple_query(stmt).await {
            record_probe(services, started.elapsed().as_secs_f64() * 1000.0, false);
            record_audit(services, desired_engine, plan.operations.len(), false);
            return Err(format!("failed on `{stmt}`: {err}"));
        }
    }

    record_probe(services, started.elapsed().as_secs_f64() * 1000.0, true);
    record_audit(services, desired_engine, plan.operations.len(), true);
    Ok(format!(
        "applied {} statements to {}",
        planned.statements.len(),
        desired_engine.as_str()
    ))
}

fn record_probe(services: &AppServices, latency_ms: f64, success: bool) {
    services.diagnostics().record_probe("db-schema-import", latency_ms, success);
    services.diagnostics().record_heartbeat("db-schema-import");
}

fn record_audit(services: &AppServices, engine: DbEngine, ops: usize, success: bool) {
    if let Ok(event) = AuditEvent::builder()
        .actor(AuditActor::System)
        .action("db::schema::import")
        .outcome(if success { AuditOutcome::Success } else { AuditOutcome::Failure })
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

