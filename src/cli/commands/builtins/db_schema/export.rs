use std::path::PathBuf;
use std::sync::Arc;

use time::OffsetDateTime;

use crate::config::AppConfig;
use crate::services::AppServices;
use crate::domain::db::DbEngine;
use crate::services::db_schema::DatabaseBlueprint;

/// Export mode determined from CLI arguments.
pub enum ExportMode {
    /// Export all tables: `db schema export all`
    All,
    /// Export specific table: `db schema export table <name>`
    Table(String),
    /// Export to specific file: `db schema export <path>`
    File(PathBuf),
}

impl ExportMode {
    pub fn parse(args: &[&str]) -> Option<Self> {
        match args.first().copied() {
            Some("all") => Some(ExportMode::All),
            Some("table") => args.get(1).map(|name| ExportMode::Table(name.to_string())),
            Some(path) if !path.is_empty() => Some(ExportMode::File(PathBuf::from(path))),
            _ => None,
        }
    }
}

pub async fn run_export(
    services: &AppServices,
    config: &Arc<AppConfig>,
    engine: Option<DbEngine>,
    mode: ExportMode,
) -> Result<String, String> {
    let db_shell = services.db_shell.clone();
    
    // Determine output path based on mode
    let (path, filter_table) = match mode {
        ExportMode::All => {
            let dir = PathBuf::from(&config.db.schema.export_dir);
            std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create export dir: {e}"))?;
            let timestamp = format_timestamp();
            let filename = format!("schema_{timestamp}.mdj");
            (dir.join(filename), None)
        }
        ExportMode::Table(ref name) => {
            let dir = PathBuf::from(&config.db.schema.export_dir);
            std::fs::create_dir_all(&dir).map_err(|e| format!("failed to create export dir: {e}"))?;
            let timestamp = format_timestamp();
            let filename = format!("{name}_{timestamp}.mdj");
            (dir.join(filename), Some(name.clone()))
        }
        ExportMode::File(path) => {
            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {e}"))?;
                }
            }
            (path, None)
        }
    };

    // Snapshot the schema
    let blueprint = DatabaseBlueprint::snapshot(db_shell, engine)
        .await
        .map_err(|e| e.to_string())?;

    // Filter to specific table if requested
    let blueprint = if let Some(table_name) = filter_table {
        blueprint.filter_table(&table_name)
            .ok_or_else(|| format!("table '{}' not found", table_name))?
    } else {
        blueprint
    };

    // Export to StarUML format
    blueprint.export_staruml(&path)
        .await
        .map_err(|e| e.to_string())?;

    Ok(format!("exported schema to {}", path.display()))
}

pub fn parse_engine(arg: Option<&str>) -> Option<DbEngine> {
    arg.and_then(|v| v.parse::<DbEngine>().ok())
}

fn format_timestamp() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

