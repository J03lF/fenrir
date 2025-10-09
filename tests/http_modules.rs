// Integration-style tests for HTTP module management endpoints.
// These mirror the helpers from integration suite but live as a standalone test crate
// to avoid filesystem permission issues in the harness.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use semver::{Version, VersionReq};
use serde_json::{json, Value};
use tower::ServiceExt;

use fenrir::audit::{AuditLog, InMemoryAuditLog};
use fenrir::domain::db::{
    DbAdminPort, DbEngine, DbExecutionResult, DbResult, DbTable, DbTableSchema,
};
use fenrir::domain::module::{
    InstalledModule, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum, ModuleId,
    ModuleInstallResult, ModuleInstallStatus, ModuleManifest, ModuleRegistryError,
    ModuleRegistryPort, ModuleSearchQuery, ModuleStorageError, ModuleStoragePort, ModuleSummary,
    ModuleVerifierPort, ModuleVersion,
};
use fenrir::infra::http::router_with_dependencies;
use fenrir::infra::storage::memory::{InMemoryTicketRepository, InMemoryUserRepository};
use fenrir::security::auth::{ControlPlaneAuthorizer, Role};
use fenrir::services::db_shell::DbShellService;
use fenrir::services::scheduler::SchedulerService;
use fenrir::services::{AppServices, ModuleService, ServiceRegistry, TicketService, UserService};

struct DummyDb;

#[async_trait]
impl DbAdminPort for DummyDb {
    async fn ping(&self) -> DbResult<()> {
        Ok(())
    }

    async fn simple_query(&self, _statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        Ok(vec![])
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        Ok(vec![])
    }

    async fn describe_table(&self, _table: &str) -> DbResult<DbTableSchema> {
        Err(fenrir::domain::db::DbError::NotImplemented {
            message: "describe_table".into(),
        })
    }
}

fn build_authorizer() -> Arc<ControlPlaneAuthorizer> {
    Arc::new(ControlPlaneAuthorizer::new(vec![
        (Role::Admin, "admin-token".to_string()),
        (Role::Operator, "operator-token".to_string()),
        (Role::Viewer, "viewer-token".to_string()),
    ]))
}

fn build_services(registry: Arc<ServiceRegistry>) -> Arc<AppServices> {
    let mut adapters: BTreeMap<DbEngine, Arc<dyn DbAdminPort>> = BTreeMap::new();
    adapters.insert(DbEngine::Sqlite, Arc::new(DummyDb));
    let db_shell = Arc::new(DbShellService::new(DbEngine::Sqlite, adapters).unwrap());
    let scheduler = Arc::new(SchedulerService::new(Arc::clone(&registry)));
    scheduler.start();
    let ticket_repo: Arc<dyn fenrir::domain::ticket::TicketRepository> =
        Arc::new(InMemoryTicketRepository::new());
    let ticket_service = Arc::new(TicketService::new(ticket_repo));
    let user_repo: Arc<dyn fenrir::domain::user::UserRepository> =
        Arc::new(InMemoryUserRepository::new());
    let user_service = Arc::new(UserService::new(user_repo));
    let audit_log: Arc<dyn AuditLog> = Arc::new(InMemoryAuditLog::new(64));

    Arc::new(AppServices::new(
        db_shell,
        scheduler,
        ticket_service,
        user_service,
        registry,
        audit_log,
    ))
}

fn build_router(registry: Arc<ServiceRegistry>) -> (Router, Arc<AppServices>) {
    let services = build_services(Arc::clone(&registry));
    let auth = build_authorizer();
    (
        router_with_dependencies(
            registry,
            Arc::clone(&services),
            auth,
            "Fenrir",
            "0.1.0-test",
            "127.0.0.1",
            8080,
        ),
        services,
    )
}

struct StubModuleRegistry {
    versions: Mutex<BTreeMap<Version, ModuleManifest>>,
}

impl StubModuleRegistry {
    fn new(manifests: Vec<ModuleManifest>) -> Self {
        let mut versions = BTreeMap::new();
        for manifest in manifests {
            versions.insert(manifest.version.clone(), manifest);
        }
        Self {
            versions: Mutex::new(versions),
        }
    }

    fn add_manifest(&self, manifest: ModuleManifest) {
        let mut guard = self.versions.lock().unwrap();
        guard.insert(manifest.version.clone(), manifest);
    }

    fn latest(&self) -> Option<ModuleManifest> {
        let guard = self.versions.lock().unwrap();
        guard
            .iter()
            .rev()
            .next()
            .map(|(_, manifest)| manifest.clone())
    }
}

#[async_trait]
impl ModuleRegistryPort for StubModuleRegistry {
    async fn search(
        &self,
        _query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError> {
        let Some(manifest) = self.latest() else {
            return Ok(vec![]);
        };

        let summary = ModuleSummary {
            id: ModuleId::new(&manifest.id).unwrap(),
            version: ModuleVersion(manifest.version.clone()),
            title: manifest.title.clone(),
            description: manifest.description.clone(),
            tags: manifest.tags.clone(),
        };
        Ok(vec![summary])
    }

    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError> {
        let guard = self.versions.lock().unwrap();
        if let Some(version) = version {
            guard
                .get(&version.0)
                .cloned()
                .ok_or_else(|| ModuleRegistryError::NotFound {
                    module: format!("{} v{}", id, version),
                })
        } else {
            guard
                .iter()
                .rev()
                .find(|(_, manifest)| manifest.id == id.as_str())
                .map(|(_, manifest)| manifest.clone())
                .ok_or_else(|| ModuleRegistryError::NotFound {
                    module: id.to_string(),
                })
        }
    }

    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError> {
        Ok(ModuleBundle {
            manifest: manifest.clone(),
            archive: vec![],
            signature: vec![],
            checksum: vec![],
        })
    }
}

struct StubModuleStorage {
    installed: Mutex<Vec<InstalledModule>>,
}

impl StubModuleStorage {
    fn new() -> Self {
        Self {
            installed: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ModuleStoragePort for StubModuleStorage {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError> {
        Ok(self.installed.lock().unwrap().clone())
    }

    async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError> {
        Ok(self
            .installed
            .lock()
            .unwrap()
            .iter()
            .find(|item| item.manifest.id == id.as_str())
            .cloned())
    }

    async fn stage_and_activate(
        &self,
        bundle: ModuleBundle,
    ) -> Result<ModuleInstallResult, ModuleStorageError> {
        let mut guard = self.installed.lock().unwrap();
        let path = format!(
            "/modules/{}/{}",
            bundle.manifest.id, bundle.manifest.version
        );

        if let Some(existing) = guard
            .iter_mut()
            .find(|item| item.manifest.id == bundle.manifest.id)
        {
            if existing.manifest.version == bundle.manifest.version {
                return Ok(ModuleInstallResult {
                    status: ModuleInstallStatus::AlreadyCurrent,
                    manifest: existing.manifest.clone(),
                    path: existing.path.clone(),
                });
            }

            existing.manifest = bundle.manifest.clone();
            existing.installed_at = SystemTime::now();
            existing.path = path.clone();

            return Ok(ModuleInstallResult {
                status: ModuleInstallStatus::Updated,
                manifest: existing.manifest.clone(),
                path,
            });
        }

        guard.push(InstalledModule {
            manifest: bundle.manifest.clone(),
            installed_at: SystemTime::now(),
            path: path.clone(),
        });

        Ok(ModuleInstallResult {
            status: ModuleInstallStatus::Installed,
            manifest: bundle.manifest,
            path,
        })
    }

    async fn remove(&self, id: &ModuleId) -> Result<(), ModuleStorageError> {
        let mut guard = self.installed.lock().unwrap();
        guard.retain(|item| item.manifest.id != id.as_str());
        Ok(())
    }
}

struct StubModuleVerifier;

#[async_trait]
impl ModuleVerifierPort for StubModuleVerifier {
    async fn verify(
        &self,
        _bundle: &ModuleBundle,
    ) -> Result<(), fenrir::domain::module::ModuleVerificationError> {
        Ok(())
    }
}

fn sample_manifest(version: &str) -> ModuleManifest {
    ModuleManifest {
        id: "sample-module".to_string(),
        version: Version::parse(version).unwrap(),
        title: Some("Sample Module".to_string()),
        description: Some("Synthetic test module".to_string()),
        fenrir_version: Some(VersionReq::parse("^0.1").unwrap()),
        authors: vec!["tester".to_string()],
        license: Some("MIT".to_string()),
        artifact: ModuleArtifactDescriptor {
            download_url: format!("https://example.invalid/sample-module-{}.tar.gz", version),
            checksum: ModuleChecksum {
                algorithm: fenrir::domain::module::ChecksumAlgorithm::Sha256,
                hash: "deadbeef".to_string(),
            },
            content_type: Some("application/gzip".to_string()),
            size_bytes: Some(42),
        },
        signature: fenrir::domain::module::ModuleSignatureDescriptor {
            algorithm: fenrir::domain::module::SignatureAlgorithm::Ed25519,
            key_id: "test-key".to_string(),
            signature: "cafebabe".to_string(),
        },
        tags: vec!["test".to_string()],
        published_at: Some(1_700_000_000),
    }
}

fn attach_module_service(
    services: &Arc<AppServices>,
    registry: Arc<StubModuleRegistry>,
    storage: Arc<StubModuleStorage>,
) {
    let verifier: Arc<dyn ModuleVerifierPort> = Arc::new(StubModuleVerifier);
    let registry_port: Arc<dyn ModuleRegistryPort> = registry;
    let storage_port: Arc<dyn ModuleStoragePort> = storage;

    let module_service = Arc::new(ModuleService::new(registry_port, storage_port, verifier));

    services.attach_module_service(module_service).unwrap();
}

fn bearer_request(method: Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn bearer_json_request(method: Method, uri: &str, token: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn modules_available_requires_auth() {
    let registry = Arc::new(ServiceRegistry::new());
    let (router, services) = build_router(Arc::clone(&registry));
    let manifest = sample_manifest("0.1.0");
    let registry_stub = Arc::new(StubModuleRegistry::new(vec![manifest]));
    let storage_stub = Arc::new(StubModuleStorage::new());
    attach_module_service(&services, Arc::clone(&registry_stub), storage_stub);

    let response = router
        .clone()
        .oneshot(
            Request::get("/modules/available")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn modules_available_lists_registry_entries() {
    let registry = Arc::new(ServiceRegistry::new());
    let (router, services) = build_router(Arc::clone(&registry));
    let manifest = sample_manifest("0.2.0");
    let registry_stub = Arc::new(StubModuleRegistry::new(vec![manifest]));
    let storage_stub = Arc::new(StubModuleStorage::new());
    attach_module_service(&services, Arc::clone(&registry_stub), storage_stub);

    let response = router
        .clone()
        .oneshot(bearer_request(
            Method::GET,
            "/modules/available",
            "viewer-token",
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["modules"].as_array().unwrap().len(), 1);
    assert_eq!(payload["modules"][0]["id"], "sample-module");
    assert_eq!(payload["modules"][0]["version"], "0.2.0");
}

#[tokio::test]
async fn modules_install_and_update_flow() {
    let registry = Arc::new(ServiceRegistry::new());
    let (router, services) = build_router(Arc::clone(&registry));
    let registry_stub = Arc::new(StubModuleRegistry::new(vec![sample_manifest("0.3.0")]));
    let storage_stub = Arc::new(StubModuleStorage::new());
    attach_module_service(
        &services,
        Arc::clone(&registry_stub),
        Arc::clone(&storage_stub),
    );

    // Install initial version
    let response = router
        .clone()
        .oneshot(bearer_json_request(
            Method::POST,
            "/modules/install",
            "operator-token",
            json!({"module_id": "sample-module"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "installed");

    // Add an updated version and trigger module update
    registry_stub.add_manifest(sample_manifest("0.4.0"));
    let response = router
        .clone()
        .oneshot(bearer_json_request(
            Method::POST,
            "/modules/update",
            "operator-token",
            json!({"module_id": "sample-module", "fenrir_version": "0.1.0"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["results"].as_array().unwrap()[0]["status"],
        "updated"
    );

    // Ensure installed list reflects new version
    let response = router
        .clone()
        .oneshot(bearer_request(
            Method::GET,
            "/modules/installed",
            "viewer-token",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["modules"].as_array().unwrap().len(), 1);
    assert_eq!(payload["modules"][0]["manifest"]["version"], "0.4.0");

    // Uninstall module
    let response = router
        .clone()
        .oneshot(bearer_request(
            Method::DELETE,
            "/modules/sample-module",
            "operator-token",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Confirm removal
    let response = router
        .clone()
        .oneshot(bearer_request(
            Method::GET,
            "/modules/installed",
            "viewer-token",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["modules"].as_array().unwrap().is_empty());
}
