use super::*;
use crate::domain::module::{
    ChecksumAlgorithm, ModuleBundle, ModuleInstallResult, ModuleInstallSource,
};
use crate::domain::module::{
    InstalledModule, ModuleArtifactDescriptor, ModuleChecksum, ModuleManifest,
    ModuleSignatureDescriptor, SignatureAlgorithm,
};
use crate::domain::module::{ModuleStorageError, ModuleStoragePort};
use async_trait::async_trait;
use semver::Version;
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

struct TestStorage {
    installed: InstalledModule,
}

#[async_trait]
impl ModuleStoragePort for TestStorage {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
        Ok(vec![self.installed.clone()])
    }

    async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError> {
        if &self.installed.manifest.id == id.as_str() {
            Ok(Some(self.installed.clone()))
        } else {
            Ok(None)
        }
    }

    async fn stage_and_activate(
        &self,
        _bundle: ModuleBundle,
        _source: ModuleInstallSource,
    ) -> Result<ModuleInstallResult, ModuleStorageError> {
        Err(ModuleStorageError::InvalidState(
            "stage not implemented in test".to_string(),
        ))
    }

    async fn remove(&self, _id: &ModuleId) -> Result<(), ModuleStorageError> {
        Err(ModuleStorageError::InvalidState(
            "remove not implemented in test".to_string(),
        ))
    }
}

fn test_manifest() -> InstalledModule {
    InstalledModule {
        manifest: ModuleManifest {
            id: "fenrir-api".to_string(),
            version: Version::parse("1.2.3").unwrap(),
            title: None,
            description: None,
            fenrir_version: None,
            authors: vec![],
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: String::new(),
                checksum: ModuleChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    hash: String::new(),
                },
                content_type: None,
                size_bytes: None,
            },
            signature: ModuleSignatureDescriptor {
                algorithm: SignatureAlgorithm::Ed25519,
                key_id: String::new(),
                signature: String::new(),
            },
            tags: vec![],
            published_at: None,
        },
        installed_at: SystemTime::now(),
        path: String::from("/dev/null"),
        source: ModuleInstallSource::Distribution,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn load_state_reattaches_running_process() {
    let tmp_dir = std::env::temp_dir().join(format!("fenrir-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let log_dir = tmp_dir.join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("fenrir-api.log");
    std::fs::write(&log_file, b"boot log").unwrap();

    let storage: Arc<dyn ModuleStoragePort> = Arc::new(TestStorage {
        installed: test_manifest(),
    });
    let runtime = ProcessModuleRuntime::new(Arc::clone(&storage), tmp_dir.clone());

    let state = PersistedModuleState {
        module_id: "fenrir-api".into(),
        version: "1.2.3".into(),
        pid: std::process::id(),
        port: Some(8080),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        restart_count: 2,
        log_file: log_file.display().to_string(),
    };

    let state_file = tmp_dir.join(STATE_FILE_NAME);
    tokio::fs::write(&state_file, serde_json::to_string(&vec![state]).unwrap())
        .await
        .unwrap();

    runtime.load_state().await.expect("state loads");

    let module_id = ModuleId::new("fenrir-api").unwrap();
    let info = runtime.status(&module_id).await.expect("status available");
    assert!(matches!(info.status, ModuleRuntimeStatus::Running));
    assert_eq!(info.pid, Some(std::process::id()));
    assert_eq!(info.port, Some(8080));
    assert_eq!(info.restart_count, 2);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
