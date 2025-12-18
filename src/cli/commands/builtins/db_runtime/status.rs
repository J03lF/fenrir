use crate::cli::output::CliOutput;
use crate::services::app::AppServices;

pub fn run_status(services: &AppServices, tail: usize) -> CliOutput {
    if let Some(status) = services.db_runtime_status() {
        let mut out = serde_json::Map::new();
        out.insert("engine".into(), serde_json::Value::String(status.engine.as_str().into()));
        out.insert("running".into(), serde_json::Value::Bool(status.running));
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

