use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context};

pub fn backup_sqlite(
    db_path: &Path,
    backup_dir: &Path,
    label: Option<&str>,
) -> anyhow::Result<PathBuf> {
    if !db_path.exists() {
        return Err(anyhow!("sqlite db file not found: {}", db_path.display()));
    }
    fs::create_dir_all(backup_dir)
        .with_context(|| format!("create backup dir {}", backup_dir.display()))?;
    let filename = label
        .map(|l| format!("sqlite-backup-{l}.db"))
        .unwrap_or_else(|| {
            format!(
                "sqlite-backup-{}.db",
                chrono::Utc::now().format("%Y%m%d%H%M%S")
            )
        });
    let target = backup_dir.join(filename);
    fs::copy(db_path, &target).with_context(|| format!("copy to {}", target.display()))?;
    Ok(target)
}

pub fn restore_sqlite(backup_file: &Path, db_path: &Path) -> anyhow::Result<()> {
    if !backup_file.exists() {
        return Err(anyhow!("backup file not found: {}", backup_file.display()));
    }
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create runtime dir {}", parent.display()))?;
    }
    fs::copy(backup_file, db_path).with_context(|| format!("restore to {}", db_path.display()))?;
    Ok(())
}

/// Backup postgres with optional Unix socket support and password authentication
#[allow(clippy::too_many_arguments)]
pub fn backup_postgres_with_auth(
    data_dir: &Path,
    backup_dir: &Path,
    pg_basebackup: &Path,
    host: &str,
    port: Option<u16>,
    user: &str,
    password: Option<&str>,
    label: Option<&str>,
) -> anyhow::Result<PathBuf> {
    if !data_dir.exists() {
        return Err(anyhow!(
            "postgres data dir not found: {}",
            data_dir.display()
        ));
    }
    fs::create_dir_all(backup_dir)
        .with_context(|| format!("create backup dir {}", backup_dir.display()))?;
    let label_owned = label
        .map(|s| s.to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y%m%d%H%M%S").to_string());
    let target_dir = backup_dir.join(format!("postgres-{}", label_owned));

    let mut cmd = Command::new(pg_basebackup);
    cmd.arg("-D")
        .arg(&target_dir)
        .arg("-X")
        .arg("stream")
        .arg("-c")
        .arg("fast")
        .arg("-h")
        .arg(host)
        .arg("-U")
        .arg(user);

    // Add port if specified (not needed for Unix socket with default port)
    if let Some(p) = port {
        // Only add port for TCP connections (host is IP address)
        if !host.starts_with('/') {
            cmd.arg("-p").arg(p.to_string());
        }
    }

    // Set PGPASSWORD for authentication
    if let Some(pw) = password {
        cmd.env("PGPASSWORD", pw);
    }

    let output = cmd
        .output()
        .with_context(|| format!("spawn pg_basebackup at {}", pg_basebackup.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("pg_basebackup failed: {}", stderr));
    }
    Ok(target_dir)
}

pub fn restore_postgres(backup_dir: &Path, data_dir: &Path) -> anyhow::Result<()> {
    if !backup_dir.exists() {
        return Err(anyhow!("backup dir not found: {}", backup_dir.display()));
    }
    if data_dir.exists() {
        fs::remove_dir_all(data_dir)
            .with_context(|| format!("remove existing data dir {}", data_dir.display()))?;
    }
    copy_dir_recursive(backup_dir, data_dir)?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("create dir {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}
