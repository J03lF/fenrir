use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn list_migration_files(engine: &str) -> Result<Vec<String>> {
    let dir = format!("migrations/{engine}");
    let path = Path::new(&dir);
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
