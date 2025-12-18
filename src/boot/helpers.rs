use std::path::{Path, PathBuf};

pub(crate) fn resolve_storage_path(base: &Path, configured: &str) -> PathBuf {
    let candidate = PathBuf::from(configured);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(configured)
    }
}
