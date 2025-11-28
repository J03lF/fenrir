use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::utils::messages::boot::helpers as helper_messages;

pub(crate) fn resolve_runtime_dir() -> PathBuf {
    if let Ok(dir) = env::var("FENRIR_RUNTIME_DIR") {
        let candidate = PathBuf::from(dir);
        if ensure_dir(&candidate) {
            return candidate;
        }
    }

    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("tmp"));
        }
        candidates.push(cwd.join("tmp"));
    }

    for candidate in candidates {
        if ensure_dir(&candidate) {
            return candidate;
        } else {
            debug!(
                path = ?candidate,
                "{}",
                helper_messages::RUNTIME_DIR_CANDIDATE_FAILED
            );
        }
    }

    let fallback = env::temp_dir().join("fenrir-runtime");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

pub(crate) fn resolve_storage_path(base: &Path, configured: &str) -> PathBuf {
    let candidate = PathBuf::from(configured);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(configured)
    }
}

fn ensure_dir(path: &Path) -> bool {
    fs::create_dir_all(path).is_ok()
}
