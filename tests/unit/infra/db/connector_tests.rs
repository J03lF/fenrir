//! DB connector (step 5.6): the host classifies statements itself (CTE gap,
//! task 7), every request carries its pool key and access mode, and the
//! socket speaks both the legacy one-shot protocol (old kits) and framed
//! persistent connections (new kits).

use super::*;
use std::sync::Mutex;

use crate::config::{
    HttpSecuritySection, IdentitySection, JwtConfig, KdfConfig, SecuritySection,
    ServiceTokenSection, SessionSection,
};
use crate::domain::db::{
    DbAccess, DbAdminPort, DbCallContext, DbEngine, DbExecutionResult, DbResult, DbResultSet,
    DbTable, DbTableSchema, DbValue,
};
use crate::security::manager::{NoopAuditSink, SecurityManager};
use crate::security::service::{ServiceRole, ServiceScope};
use crate::security::service_tokens::{DelegatedActor, DelegatedTokenRequest};
use crate::services::db_shell::DbShellService;
use crate::services::ServiceDiagnostics;
use fenrir_module_kit::{
    ConnectorEndpoint, ControlPlaneEnvironment, ControlPlaneTlsEnvironment, DbConnectorClient,
    DbConnectorCommand, DbConnectorIntent, DbConnectorResultView, ModuleEnvironment,
    ServiceTokenLease,
};

/// Answers every statement with one row and records who asked for what.
#[derive(Default)]
struct RecordingAdapter {
    calls: Mutex<Vec<(DbCallContext, String)>>,
}

impl RecordingAdapter {
    fn record(&self, ctx: &DbCallContext, statement: &str) {
        self.calls
            .lock()
            .unwrap()
            .push((ctx.clone(), statement.to_string()));
    }

    fn calls(&self) -> Vec<(DbCallContext, String)> {
        self.calls.lock().unwrap().clone()
    }

    fn row() -> Vec<DbExecutionResult> {
        vec![DbExecutionResult::ResultSet(DbResultSet::new(
            vec!["ok".to_string()],
            vec![vec!["1".to_string()]],
        ))]
    }
}

#[async_trait::async_trait]
impl DbAdminPort for RecordingAdapter {
    async fn ping(&self) -> DbResult<()> {
        Ok(())
    }
    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        self.simple_query_in(&DbCallContext::host(), statement)
            .await
    }
    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        Ok(Vec::new())
    }
    async fn describe_table(&self, _table: &str) -> DbResult<DbTableSchema> {
        Err(crate::domain::db::DbError::invalid_input("not here"))
    }
    async fn prepared_query(
        &self,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        self.prepared_query_in(&DbCallContext::host(), statement, params)
            .await
    }
    async fn prepared_execute(&self, statement: &str, params: &[DbValue]) -> DbResult<u64> {
        self.prepared_execute_in(&DbCallContext::host(), statement, params)
            .await
    }
    async fn simple_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
    ) -> DbResult<Vec<DbExecutionResult>> {
        self.record(ctx, statement);
        Ok(Self::row())
    }
    async fn prepared_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        _params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        self.record(ctx, statement);
        Ok(Self::row())
    }
    async fn prepared_execute_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        _params: &[DbValue],
    ) -> DbResult<u64> {
        self.record(ctx, statement);
        Ok(1)
    }
}

fn security_config() -> SecuritySection {
    SecuritySection {
        kdf: KdfConfig {
            algorithm: "argon2id".to_string(),
            version: 1,
            memory_mib: 8,
            iterations: 2,
            parallelism: 1,
            salt_length: 16,
            output_length: 32,
        },
        jwt: JwtConfig {
            issuer: "fenrir".to_string(),
            audience: "aud".to_string(),
            exp_seconds: 3600,
        },
        allowed_ciphers: vec!["AES-GCM".to_string()],
        http: HttpSecuritySection {
            control_tokens: Vec::new(),
        },
        session: SessionSection {
            lifetime_seconds: 60,
            idle_timeout_seconds: 60,
            cleanup_interval_seconds: 60,
        },
        service_tokens: ServiceTokenSection {
            lifetime_seconds: 600,
            idle_timeout_seconds: 600,
            cleanup_interval_seconds: 600,
            refresh_grace_seconds: 600,
        },
        identity: IdentitySection::default(),
        password_policy: Default::default(),
    }
}

struct Fixture {
    service: Arc<DbConnectorService>,
    security: Arc<SecurityManager>,
    adapter: Arc<RecordingAdapter>,
}

fn fixture() -> Fixture {
    let adapter = Arc::new(RecordingAdapter::default());
    let mut adapters: std::collections::BTreeMap<DbEngine, Arc<dyn DbAdminPort>> =
        std::collections::BTreeMap::new();
    adapters.insert(DbEngine::Postgres, adapter.clone());
    let db_shell = Arc::new(DbShellService::new(DbEngine::Postgres, adapters).unwrap());
    let security =
        Arc::new(SecurityManager::new(&security_config(), Arc::new(NoopAuditSink)).unwrap());
    let service = Arc::new(DbConnectorService::new(
        db_shell,
        Arc::clone(&security),
        Arc::new(ServiceDiagnostics::new()),
    ));
    Fixture {
        service,
        security,
        adapter,
    }
}

fn token(security: &SecurityManager, service_id: &str, scopes: &[&str]) -> String {
    security
        .issue_service_token(
            DelegatedTokenRequest::new(
                DelegatedActor::Service {
                    service_id: service_id.to_string(),
                    role: ServiceRole::Write,
                },
                "tenant-1",
            )
            .with_scopes(
                scopes
                    .iter()
                    .map(|scope| ServiceScope::new(*scope).unwrap())
                    .collect(),
            ),
        )
        .unwrap()
        .token
}

fn request(token: &str, sql: &str, intent: DbConnectorIntent) -> DbConnectorRequest {
    DbConnectorRequest {
        token: token.to_string(),
        engine: None,
        intent: Some(intent),
        command: DbConnectorCommand::Prepared {
            statement: sql.to_string(),
            params: Vec::new(),
        },
        tenant: None,
    }
}

// --- Task 7: the CTE gap -----------------------------------------------------

#[tokio::test]
async fn a_read_only_module_cannot_write_through_a_cte() {
    let f = fixture();
    let read_token = token(&f.security, "module:reader::core", &["db:read"]);
    for sql in [
        "WITH x AS (UPDATE athene_tickets SET title = 'x' RETURNING id) SELECT * FROM x",
        "WITH x AS (INSERT INTO athene_tickets (id) VALUES ($1) RETURNING id) SELECT id FROM x",
        "WITH x AS (DELETE FROM athene_tickets RETURNING id) SELECT count(*) FROM x",
        "SELECT * FROM athene_tickets FOR UPDATE",
        "SELECT set_config('search_path', 'evil', false)",
    ] {
        // The intent the client sends is not trusted: Read here.
        let response = f
            .service
            .execute(request(&read_token, sql, DbConnectorIntent::Read))
            .await;
        assert!(!response.ok, "{sql} passed with db:read only");
        assert!(
            response
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("db:write"),
            "{sql}: {:?}",
            response.error
        );
    }
    assert!(f.adapter.calls().is_empty(), "nothing reached the database");
}

#[tokio::test]
async fn a_pure_read_cte_still_works_and_runs_read_only() {
    let f = fixture();
    let read_token = token(&f.security, "module:reader::core", &["db:read"]);
    let response = f
        .service
        .execute(request(
            &read_token,
            "WITH x AS (SELECT id FROM athene_tickets) SELECT * FROM x",
            DbConnectorIntent::Read,
        ))
        .await;
    assert!(response.ok, "{:?}", response.error);
    let calls = f.adapter.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.access, DbAccess::ReadOnly);
    assert_eq!(
        calls[0].0.pool.as_str(),
        "service:module:reader::core@tenant-1"
    );
}

#[tokio::test]
async fn a_write_cte_with_db_write_returns_its_rows() {
    let f = fixture();
    let write_token = token(&f.security, "module:writer::core", &["db:read", "db:write"]);
    let response = f
        .service
        .execute(request(
            &write_token,
            "WITH x AS (UPDATE athene_tickets SET title = 'x' RETURNING id) SELECT * FROM x",
            DbConnectorIntent::Read,
        ))
        .await;
    assert!(response.ok, "{:?}", response.error);
    // The caller asked for rows (Read shape) and gets them, read-write.
    assert!(matches!(
        response.results.as_deref(),
        Some([DbConnectorResultView::ResultSet { .. }])
    ));
    let calls = f.adapter.calls();
    assert_eq!(calls[0].0.access, DbAccess::ReadWrite);
    let response = f
        .service
        .execute(request(
            &write_token,
            "WITH y AS (SELECT 1) UPDATE athene_tickets SET title = 'x' RETURNING id",
            DbConnectorIntent::Write,
        ))
        .await;
    assert!(response.ok, "{:?}", response.error);
}

#[tokio::test]
async fn pools_are_keyed_per_module_service() {
    let f = fixture();
    let a = token(&f.security, "module:a::core", &["db:read"]);
    let b = token(&f.security, "module:b::core", &["db:read"]);
    for token in [&a, &b] {
        let response = f
            .service
            .execute(request(token, "SELECT 1", DbConnectorIntent::Read))
            .await;
        assert!(response.ok);
    }
    let keys: Vec<String> = f
        .adapter
        .calls()
        .iter()
        .map(|(ctx, _)| ctx.pool.as_str().to_string())
        .collect();
    assert_eq!(
        keys,
        vec![
            "service:module:a::core@tenant-1".to_string(),
            "service:module:b::core@tenant-1".to_string()
        ]
    );
}

// --- Protocols on the socket -------------------------------------------------

#[tokio::test]
async fn an_old_kit_gets_the_one_shot_protocol() {
    let f = fixture();
    let read_token = token(&f.security, "module:old::core", &["db:read"]);
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let service = Arc::clone(&f.service);
    let host = tokio::spawn(async move { process_stream(server, service).await });
    // Exactly what kits before 5.6 do: JSON, shut down the write side, read
    // until EOF.
    let payload =
        serde_json::to_vec(&request(&read_token, "SELECT 1", DbConnectorIntent::Read)).unwrap();
    client.write_all(&payload).await.unwrap();
    client.shutdown().await.unwrap();
    let mut answer = Vec::new();
    client.read_to_end(&mut answer).await.unwrap();
    let response: DbConnectorResponse = serde_json::from_slice(&answer).unwrap();
    assert!(response.ok, "{:?}", response.error);
    host.await.unwrap().unwrap();
}

async fn send_frame(stream: &mut tokio::net::UnixStream, payload: &[u8]) {
    let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(payload);
    stream.write_all(&frame).await.unwrap();
}

async fn read_frame(stream: &mut tokio::net::UnixStream) -> DbConnectorResponse {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.unwrap();
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn a_framed_connection_serves_many_requests() {
    let f = fixture();
    let read_token = token(&f.security, "module:new::core", &["db:read"]);
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let service = Arc::clone(&f.service);
    let host = tokio::spawn(async move { process_stream(server, service).await });
    client
        .write_all(fenrir_module_kit::CONNECTOR_FRAMED_PREAMBLE)
        .await
        .unwrap();
    for _ in 0..5 {
        let payload =
            serde_json::to_vec(&request(&read_token, "SELECT 1", DbConnectorIntent::Read)).unwrap();
        send_frame(&mut client, &payload).await;
        assert!(read_frame(&mut client).await.ok);
    }
    // A bad request is answered with an error; the connection stays usable.
    send_frame(&mut client, b"{not json").await;
    assert!(!read_frame(&mut client).await.ok);
    let payload =
        serde_json::to_vec(&request(&read_token, "SELECT 2", DbConnectorIntent::Read)).unwrap();
    send_frame(&mut client, &payload).await;
    assert!(read_frame(&mut client).await.ok);
    drop(client);
    host.await.unwrap().unwrap();
    assert_eq!(f.adapter.calls().len(), 6);
}

#[tokio::test]
async fn an_oversized_frame_is_refused() {
    let f = fixture();
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let service = Arc::clone(&f.service);
    let host = tokio::spawn(async move { process_stream(server, service).await });
    client
        .write_all(fenrir_module_kit::CONNECTOR_FRAMED_PREAMBLE)
        .await
        .unwrap();
    client
        .write_all(&(u32::try_from(MAX_REQUEST_SIZE).unwrap() + 1).to_be_bytes())
        .await
        .unwrap();
    assert!(!read_frame(&mut client).await.ok);
    assert!(host.await.unwrap().is_err());
}

fn kit_client(path: &std::path::Path, token: &str, framed: bool) -> DbConnectorClient {
    let env = ModuleEnvironment {
        module_id: "module:new".to_string(),
        service_id: "module:new::core".to_string(),
        service_token: token.to_string(),
        connector: ConnectorEndpoint::Ipc {
            path: path.display().to_string(),
        },
        control_plane: ControlPlaneEnvironment {
            url: None,
            timeout: std::time::Duration::from_secs(1),
            retries: 0,
            backoff: std::time::Duration::from_millis(10),
            tls: ControlPlaneTlsEnvironment::default(),
        },
        service_token_lease: ServiceTokenLease::new(token, None, None, None),
    };
    DbConnectorClient::with_parts(env.connector.clone(), env.token_provider().unwrap(), framed)
}

/// The kit of this step against this host, over the real socket server:
/// framed with connection reuse, the kit's legacy mode, and the blocking API
/// of earlier kits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_and_old_kit_modes_against_the_socket_server() {
    let f = fixture();
    let read_token = token(&f.security, "module:new::core", &["db:read"]);
    let dir = tempfile::tempdir().unwrap();
    let endpoint = start_connector_server(dir.path(), Arc::clone(&f.service))
        .await
        .unwrap();
    let DbConnectorEndpoint::Ipc { path } = endpoint else {
        panic!("expected an ipc endpoint");
    };
    let framed = kit_client(&path, &read_token, true);
    for _ in 0..10 {
        let response = framed
            .execute_async(
                DbConnectorCommand::Prepared {
                    statement: "SELECT 1".to_string(),
                    params: Vec::new(),
                },
                DbConnectorIntent::Read,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(response.ok, "{:?}", response.error);
    }
    let legacy = kit_client(&path, &read_token, false);
    let response = legacy
        .execute_async(
            DbConnectorCommand::Simple {
                statement: "SELECT 1".to_string(),
            },
            DbConnectorIntent::Read,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(response.ok, "{:?}", response.error);
    let blocking = tokio::task::spawn_blocking(move || {
        legacy.execute(
            DbConnectorCommand::Simple {
                statement: "SELECT 1".to_string(),
            },
            DbConnectorIntent::Read,
            None,
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(blocking.ok, "{:?}", blocking.error);
    assert_eq!(f.adapter.calls().len(), 12);
}

/// Measurement, not a gate: the whole host side of one statement
/// (token check, scope, classification, pool, Postgres) with 1, 5 and 20
/// parallel callers. Needs `FENRIR_PG_TEST_URI_FILE`. Run with
/// `cargo test --release --lib measure_connector -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "measurement"]
async fn measure_connector_concurrency() {
    let Some(uri) = std::env::var("FENRIR_PG_TEST_URI_FILE")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
    else {
        eprintln!("skipped: FENRIR_PG_TEST_URI_FILE not set");
        return;
    };
    let adapter =
        crate::infra::db::adapters::postgres::PostgresAdapter::new(uri.trim(), None, None).unwrap();
    let mut adapters: std::collections::BTreeMap<DbEngine, Arc<dyn DbAdminPort>> =
        std::collections::BTreeMap::new();
    adapters.insert(DbEngine::Postgres, Arc::new(adapter));
    let db_shell = Arc::new(DbShellService::new(DbEngine::Postgres, adapters).unwrap());
    let security =
        Arc::new(SecurityManager::new(&security_config(), Arc::new(NoopAuditSink)).unwrap());
    let service = Arc::new(DbConnectorService::new(
        db_shell,
        Arc::clone(&security),
        Arc::new(ServiceDiagnostics::new()),
    ));
    let token = token(&security, "module:bench::core", &["db:read"]);
    let sql = "SELECT id, title FROM (VALUES ('1', 'a'), ('2', 'b')) v(id, title) WHERE id = $1";
    for callers in [1usize, 5, 20] {
        let window = std::time::Duration::from_secs(3);
        let started = std::time::Instant::now();
        let tasks: Vec<_> = (0..callers)
            .map(|_| {
                let service = Arc::clone(&service);
                let token = token.clone();
                tokio::spawn(async move {
                    let mut latencies = Vec::new();
                    while started.elapsed() < window {
                        let request = DbConnectorRequest {
                            token: token.clone(),
                            engine: None,
                            intent: Some(DbConnectorIntent::Read),
                            command: DbConnectorCommand::Prepared {
                                statement: sql.to_string(),
                                params: vec![fenrir_module_kit::DbPreparedParam {
                                    name: "id".to_string(),
                                    value: serde_json::Value::String("1".to_string()),
                                }],
                            },
                            tenant: None,
                        };
                        let t = std::time::Instant::now();
                        let response = service.execute(request).await;
                        assert!(response.ok, "{:?}", response.error);
                        latencies.push(t.elapsed());
                    }
                    latencies
                })
            })
            .collect();
        let mut all = Vec::new();
        for task in tasks {
            all.extend(task.await.unwrap());
        }
        all.sort();
        let p50 = all[all.len() / 2];
        let p95 = all[all.len() * 95 / 100];
        println!(
            "connector callers={callers:>2} stmts/s={:>8.0} p50={:>7.3}ms p95={:>7.3}ms",
            all.len() as f64 / window.as_secs_f64(),
            p50.as_secs_f64() * 1000.0,
            p95.as_secs_f64() * 1000.0
        );
    }
}
