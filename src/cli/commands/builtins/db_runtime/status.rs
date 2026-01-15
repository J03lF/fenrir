use crate::cli::output::CliOutput;
use crate::services::app::AppServices;
use crate::utils::format_offset_datetime;

pub fn run_status(services: &AppServices, tail: usize) -> CliOutput {
    if let Some(status) = services.db_runtime_status() {
        let mut out = serde_json::Map::new();
        out.insert("engine".into(), serde_json::Value::String(status.engine.as_str().into()));
        out.insert("running".into(), serde_json::Value::Bool(status.running));
        out.insert(
            "adapter_status".into(),
            serde_json::Value::String(status.adapter_status.as_str().into()),
        );
        if let Some(uri) = status.connector_uri {
            out.insert("uri".into(), serde_json::Value::String(uri));
        }
        if let Some(port) = status.port {
            out.insert("port".into(), serde_json::Value::Number(port.into()));
        }
        if let Some(pid) = status.pid {
            out.insert("pid".into(), serde_json::Value::Number(pid.into()));
        }
        if let Some(health) = status.last_health {
            out.insert(
                "last_health".into(),
                serde_json::Value::String(health.to_string()),
            );
        }
        if let Some(checkpoint) = status.last_checkpoint {
            out.insert(
                "last_checkpoint".into(),
                serde_json::Value::String(format_offset_datetime(checkpoint)),
            );
        }
        if !status.applied_migrations.is_empty() {
            out.insert(
                "applied_migrations".into(),
                serde_json::Value::Array(
                    status
                        .applied_migrations
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        if let Some(updated) = status.snapshot_updated_at {
            out.insert(
                "snapshot_updated_at".into(),
                serde_json::Value::String(format_offset_datetime(updated)),
            );
        }
        if let Some(path) = status.last_backup_path {
            out.insert("last_backup_path".into(), serde_json::Value::String(path));
        }
        if let Some(state_path) = status.last_backup_state_path {
            out.insert(
                "last_backup_state_path".into(),
                serde_json::Value::String(state_path),
            );
        }
        if tail > 0 {
            let logs = services.db_runtime_logs(tail);
            out.insert(
                "logs".into(),
                serde_json::Value::Array(logs.into_iter().map(serde_json::Value::String).collect()),
            );
        }
        CliOutput::json(out)
    } else {
        CliOutput::error("db-runtime not available (mode != embedded?)")
    }
}
