use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use axum::ServiceExt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fenrir::audit::{AuditLog, InMemoryAuditLog};
use fenrir::domain::db::{DbAdminPort, DbEngine, DbExecutionResult, DbResult, DbTable, DbTableSchema};
use fenrir::infra::http::router_with_dependencies;
use fenrir::infra::storage::memory::{InMemoryTicketRepository, InMemoryUserRepository};
use fenrir::security::auth::{ControlPlaneAuthorizer, Role};
use fenrir::services::db_shell::DbShellService;
use fenrir::services::scheduler::SchedulerService;
use fenrir::services::{
    AppServices, ManagedService, ServiceDescriptor, ServiceKind, ServiceRegistry, ServiceStatus,
    TicketService, UserService,
};

struct DummyDb;

#[async_trait::async_trait]
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

struct FakeManagedService {
    id: &'static str,
    registry: Arc<ServiceRegistry>,
    running: Mutex<bool>,
}

impl FakeManagedService {
    fn new(id: &'static str, registry: Arc<ServiceRegistry>) -> Arc<Self> {
        Arc::new(Self {
            id,
            registry,
            running: Mutex::new(false),
        })
    }
}

#[async_trait::async_trait]
impl ManagedService for FakeManagedService {
    fn id(&self) -> &'static str {
        self.id
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        let mut guard = self.running.lock().unwrap();
        if *guard {
            return Ok(false);
        }
        *guard = true;
        self.registry
            .set_status(self.id, ServiceStatus::Active, Some("gestartet".into()));
        Ok(true)
    }

    async fn stop(self: Arc<Self>, _force: bool) -> anyhow::Result<bool> {
        let mut guard = self.running.lock().unwrap();
        if !*guard {
            return Ok(false);
        }
        *guard = false;
        self.registry
            .set_status(self.id, ServiceStatus::Stopped, Some("gestoppt".into()));
        Ok(true)
    }
}

fn build_test_services(registry: Arc<ServiceRegistry>) -> Arc<AppServices> {
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

fn build_authorizer() -> Arc<ControlPlaneAuthorizer> {
    Arc::new(ControlPlaneAuthorizer::new(vec![
        (Role::Admin, "admin-token".to_string()),
        (Role::Operator, "operator-token".to_string()),
        (Role::Viewer, "viewer-token".to_string()),
    ]))
}

fn build_router(registry: Arc<ServiceRegistry>) -> (Router, Arc<AppServices>) {
    let services = build_test_services(Arc::clone(&registry));
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

fn register_service(registry: &Arc<ServiceRegistry>, descriptor: ServiceDescriptor, critical: bool) {
    let descriptor = if critical { descriptor.critical() } else { descriptor };
    registry.register(descriptor, ServiceStatus::Standby, Some("test".into()));
}

fn bearer_request(method: http::Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn bearer_json_request(method: http::Method, uri: &str, token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn denies_unauthorized_control_request() {
    let registry = Arc::new(ServiceRegistry::new());
    register_service(
        &registry,
        ServiceDescriptor::new(
            "test",
            "Test Service",
            "manual service",
            ServiceKind::Other,
        ),
        false,
    );
    let (mut router, services) = build_router(Arc::clone(&registry));
    let managed = FakeManagedService::new("test", services.registry());
    services.register_runtime_service(managed);

    let response = router
        .clone()
        .oneshot(Request::post("/services/test/start").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn allows_operator_to_start_service() {
    let registry = Arc::new(ServiceRegistry::new());
    register_service(
        &registry,
        ServiceDescriptor::new(
            "test",
            "Test Service",
            "manual service",
            ServiceKind::Other,
        ),
        false,
    );
    let (mut router, services) = build_router(Arc::clone(&registry));
    let managed = FakeManagedService::new("test", services.registry());
    services.register_runtime_service(managed);

    let response = router
        .clone()
        .oneshot(bearer_request(http::Method::POST, "/services/test/start", "operator-token"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let snapshot = registry
        .snapshot()
        .into_iter()
        .find(|svc| svc.descriptor.id == "test")
        .unwrap();
    assert_eq!(snapshot.status, ServiceStatus::Active);
}

#[tokio::test]
async fn requires_force_for_critical_service_stop() {
    let registry = Arc::new(ServiceRegistry::new());
    register_service(
        &registry,
        ServiceDescriptor::new(
            "critical",
            "Critical Service",
            "critical",
            ServiceKind::Other,
        ),
        true,
    );
    let (mut router, services) = build_router(Arc::clone(&registry));
    let managed = FakeManagedService::new("critical", services.registry());
    services.register_runtime_service(managed);
    // Start service first
    router
        .clone()
        .oneshot(bearer_request(http::Method::POST, "/services/critical/start", "operator-token"))
        .await
        .unwrap();

    let response = router
        .clone()
        .oneshot(bearer_request(http::Method::POST, "/services/critical/stop", "operator-token"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);

    let forced = router
        .oneshot(bearer_json_request(
            http::Method::POST,
            "/services/critical/stop",
            "admin-token",
            "{\"force\":true}",
        ))
        .await
        .unwrap();
    assert_eq!(forced.status(), StatusCode::OK);
}

#[tokio::test]
async fn viewer_can_list_services_when_authorized() {
    let registry = Arc::new(ServiceRegistry::new());
    register_service(
        &registry,
        ServiceDescriptor::new("svc", "Svc", "", ServiceKind::Other),
        false,
    );
    let (mut router, services) = build_router(Arc::clone(&registry));
    let managed = FakeManagedService::new("svc", services.registry());
    services.register_runtime_service(managed);

    let response = router
        .oneshot(bearer_request(http::Method::GET, "/services", "viewer-token"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn viewer_can_fetch_scheduler_jobs() {
    let registry = Arc::new(ServiceRegistry::new());
    register_service(
        &registry,
        ServiceDescriptor::new("svc", "Svc", "", ServiceKind::Other),
        false,
    );
    let (mut router, services) = build_router(Arc::clone(&registry));
    let managed = FakeManagedService::new("svc", services.registry());
    services.register_runtime_service(managed);

    let response = router
        .oneshot(bearer_request(
            http::Method::GET,
            "/scheduler/jobs",
            "viewer-token",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
