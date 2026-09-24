use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

const MIGRATIONS_ROOT: &str = "infra/db/migrations";
const MODULE_RUNTIME_MANIFEST: &str = ".fenrir/runtime.toml";

pub fn list_migration_files(engine: &str) -> Result<Vec<String>> {
    let dir = migration_dir(engine);
    list_sql_files_in(&dir)
}

pub fn migration_dir(engine: &str) -> PathBuf {
    Path::new(MIGRATIONS_ROOT).join(engine)
}

/// Describes a module that declares DB migrations.
#[derive(Debug, Clone)]
pub struct ModuleMigrationSource {
    pub module_id: String,
    pub migrations_dir: PathBuf,
}

/// Scan an install directory for modules that declare `[migrations]` in
/// their `.fenrir/runtime.toml` and return the resolved migration paths.
pub fn discover_module_migrations(install_dir: &Path) -> Vec<ModuleMigrationSource> {
    let entries = match fs::read_dir(install_dir) {
        Ok(rd) => rd,
        Err(_) => return vec![],
    };
    let mut sources = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let module_id = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) if !name.starts_with('.') => name.to_string(),
            _ => continue,
        };
        let manifest_path = path.join(MODULE_RUNTIME_MANIFEST);
        if !manifest_path.exists() {
            continue;
        }
        let raw = match fs::read_to_string(&manifest_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(mig_dir) = parse_migrations_dir(&raw) {
            let resolved = path.join(&mig_dir);
            if resolved.is_dir() {
                sources.push(ModuleMigrationSource {
                    module_id,
                    migrations_dir: resolved,
                });
            }
        }
    }
    sources.sort_by(|a, b| a.module_id.cmp(&b.module_id));
    sources
}

/// List `.sql` files sorted lexicographically from the given directory.
pub fn list_sql_files_in(dir: &Path) -> Result<Vec<String>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut files: Vec<String> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file() && e.path().extension().is_some_and(|ext| ext == "sql"))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    files.sort();
    Ok(files)
}

/// Minimal TOML parse: extract `[migrations] dir = "..."` without pulling
/// in the full `RuntimeManifest` dependency (which is private to process.rs).
fn parse_migrations_dir(toml_str: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Stub {
        migrations: Option<MigSection>,
    }
    #[derive(serde::Deserialize)]
    struct MigSection {
        #[serde(default = "default_dir")]
        dir: String,
    }
    fn default_dir() -> String {
        "migrations".to_string()
    }
    let stub: Stub = toml::from_str(toml_str).ok()?;
    stub.migrations.map(|m| m.dir)
}
