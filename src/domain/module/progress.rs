use std::sync::Arc;

/// Progress event during module operations
#[derive(Debug, Clone)]
pub enum ModuleProgress {
    /// Download started
    DownloadStarted {
        module_id: String,
        total_bytes: Option<u64>,
    },
    /// Download progress update
    DownloadProgress {
        module_id: String,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    /// Download completed
    DownloadCompleted { module_id: String, total_bytes: u64 },
    /// Verification started
    VerificationStarted { module_id: String },
    /// Installation started
    InstallationStarted { module_id: String },
}

pub type ProgressCallback = Arc<dyn Fn(ModuleProgress) + Send + Sync>;
