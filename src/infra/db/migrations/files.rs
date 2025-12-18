use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

const MIGRATIONS_ROOT: &str = "infra/db/migrations";

pub fn list_migration_files(engine: &str) -> Result<Vec<String>> {
    let dir = migration_dir(engine);
    let path = dir.as_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let mut files: Vec<String> = fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    files.sort();
    Ok(files)
}

pub fn migration_dir(engine: &str) -> PathBuf {
    Path::new(MIGRATIONS_ROOT).join(engine)
}
