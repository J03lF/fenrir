use super::*;
use crate::domain::module::{
    InstalledModule, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum, ModuleInstallResult,
    ModuleInstallSource, ModuleManifest, ModuleSignatureDescriptor, ModuleStorageError,
    ModuleStoragePort, SignatureAlgorithm,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::SystemTime;

struct TestStorage {
    installed: Option<InstalledModule>,
}

#[async_trait]
impl ModuleStoragePort for TestStorage {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
        Ok(self.installed.iter().cloned().collect())
    }

    async fn load(&self, _id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError> {
        Ok(self.installed.clone())
    }

    async fn stage_and_activate(
        &self,
        _bundle: ModuleBundle,
        _source: ModuleInstallSource,
    ) -> Result<ModuleInstallResult, ModuleStorageError> {
        Err(ModuleStorageError::InvalidState("not implemented".into()))
    }

    async fn remove(&self, _id: &ModuleId) -> Result<(), ModuleStorageError> {
        Ok(())
    }
}

fn test_manifest() -> InstalledModule {
    InstalledModule {
        manifest: ModuleManifest {
            id: "test-module".to_string(),
            version: semver::Version::parse("0.1.0").unwrap(),
            title: None,
            description: None,
            fenrir_version: None,
            authors: vec![],
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: String::new(),
                checksum: ModuleChecksum {
                    algorithm: crate::domain::module::ChecksumAlgorithm::Sha256,
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
        path: "test".into(),
        source: ModuleInstallSource::Distribution,
    }
}

#[tokio::test]
async fn start_stop_cycle_tracks_state() {
    let storage = Arc::new(TestStorage {
        installed: Some(test_manifest()),
    });
    let runtime = InProcessModuleRuntime::new(storage);
    let module_id = ModuleId::new("test-module").unwrap();

    let info = runtime
        .start(ModuleStartConfig {
            module_id: module_id.clone(),
            port: Some(8080),
            env_vars: vec![("FOO".to_string(), "bar".to_string())],
            auto_restart: false,
        })
        .await
        .expect("start succeeds");
    assert_eq!(info.port, Some(8080));
    assert!(matches!(info.status, ModuleRuntimeStatus::Running));

    runtime.stop(&module_id).await.unwrap();
    let status = runtime.status(&module_id).await.unwrap();
    assert!(matches!(status.status, ModuleRuntimeStatus::Stopped));

    let logs = runtime.logs(&module_id, None).await.unwrap();
    assert!(logs.iter().any(|line| line.contains("stopped")));
}

#[tokio::test]
async fn restart_increments_counter() {
    let storage = Arc::new(TestStorage {
        installed: Some(test_manifest()),
    });
    let runtime = InProcessModuleRuntime::new(storage);
    let module_id = ModuleId::new("test-module").unwrap();

    runtime
        .start(ModuleStartConfig {
            module_id: module_id.clone(),
            port: None,
            env_vars: vec![],
            auto_restart: false,
        })
        .await
        .unwrap();
    let info = runtime.restart(&module_id).await.unwrap();
    assert_eq!(info.restart_count, 1);
}
