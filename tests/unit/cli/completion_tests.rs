use super::{parse_state, ContextualCompleter};
use crate::audit::InMemoryAuditLog;
use crate::cli::commands::builtins;
use crate::cli::commands::registry::{CliDependencies, ShellEnvironment};
use crate::config::{
    AppConfig, AppSection, AuditSection, AuditStorageSection, CliSection, DbConnectionSettings,
    DbConnections, DbPoolSettings, DbSection, HttpConfig, HttpSecuritySection, HttpTlsConfig,
    IdentitySection, JwtConfig, KdfConfig, ModuleDevSourcesSection, ModuleRegistrySection,
    ModuleRegistryTlsSection, ModuleRuntimeSection, ModuleStorageSection, ModuleTrustSection,
    ModulesSection, SecuritySection, ServerSection, ServiceTokenSection, SessionSection, SshConfig,
    SshTlsConfig, TelemetryHealthSection, TelemetryMetricsSection, TelemetrySection,
    TelemetrySystemSection, TelemetryTracingSection,
};
use crate::domain::db::{
    DbAdminPort, DbEngine, DbExecutionResult, DbResult, DbTable, DbTableSchema, DbValue,
};
use crate::services::ServiceRegistry;
use crate::services::{AppServices, DbShellService, SchedulerService};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

struct DummyDbAdapter;

#[async_trait]
impl DbAdminPort for DummyDbAdapter {
    async fn ping(&self) -> DbResult<()> {
        Ok(())
    }

    async fn simple_query(&self, _statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        Err(crate::domain::db::DbError::NotImplemented {
            message: "simple_query not available in DummyDbAdapter".to_string(),
        })
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        Ok(Vec::new())
    }

    async fn describe_table(&self, _table: &str) -> DbResult<DbTableSchema> {
        Err(crate::domain::db::DbError::NotImplemented {
            message: "describe_table not available in DummyDbAdapter".to_string(),
        })
    }

    async fn prepared_query(
        &self,
        _statement: &str,
        _params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        Err(crate::domain::db::DbError::NotImplemented {
            message: "prepared_query not available in DummyDbAdapter".to_string(),
        })
    }

    async fn prepared_execute(&self, _statement: &str, _params: &[DbValue]) -> DbResult<u64> {
        Err(crate::domain::db::DbError::NotImplemented {
            message: "prepared_execute not available in DummyDbAdapter".to_string(),
        })
    }
}

fn test_config() -> Arc<AppConfig> {
    let mut connections = DbConnections::default();
    connections.postgres = Some(DbConnectionSettings {
        uri: "postgres://test".to_string(),
        pool: DbPoolSettings {
            max: Some(8),
            timeout_ms: Some(1000),
        },
    });

    Arc::new(AppConfig {
        app: AppSection {
            name: "fenrir-test".to_string(),
            version: "0.0.0".to_string(),
        },
        server: ServerSection {
            enable_http: false,
            enable_grpc: false,
            ssh: SshConfig {
                host: "127.0.0.1".to_string(),
                port: 2222,
                user: "tester".to_string(),
                server_name: "fenrir-test".to_string(),
                host_key_path: "keys/test_ed25519".to_string(),
                idle_close_seconds: Some(30),
                tls: SshTlsConfig::default(),
            },
            http: HttpConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
                tls: HttpTlsConfig::default(),
            },
            grpc: None,
        },
        security: SecuritySection {
            kdf: KdfConfig {
                algorithm: "argon2id".to_string(),
                version: 1,
                memory_mib: 64,
                iterations: 3,
                parallelism: 2,
                salt_length: 16,
                output_length: 32,
            },
            jwt: JwtConfig {
                issuer: "fenrir".to_string(),
                audience: "fenrir".to_string(),
                exp_seconds: 3600,
            },
            allowed_ciphers: vec!["AES-GCM".to_string(), "XChaCha20-Poly1305".to_string()],
            http: HttpSecuritySection {
                control_tokens: Vec::new(),
            },
            session: SessionSection {
                lifetime_seconds: 3600,
                idle_timeout_seconds: 900,
                cleanup_interval_seconds: 300,
            },
            service_tokens: ServiceTokenSection {
                lifetime_seconds: 900,
                idle_timeout_seconds: 300,
                cleanup_interval_seconds: 60,
            },
            identity: IdentitySection::default(),
        },
        db: DbSection {
            default_engine: "postgres".to_string(),
            connections,
        },
        telemetry: TelemetrySection {
            tracing: TelemetryTracingSection {
                level: "info".to_string(),
            },
            metrics: TelemetryMetricsSection {
                enabled: false,
                exporter: None,
            },
            health: TelemetryHealthSection { enabled: false },
            system: TelemetrySystemSection {
                enabled: true,
                interval_ms: Some(5000),
            },
        },
        audit: AuditSection {
            enabled: false,
            buffer_capacity: None,
            storage: AuditStorageSection::default(),
        },
        cli: CliSection {
            prompt_theme: "default".to_string(),
        },
        modules: ModulesSection {
            registry: ModuleRegistrySection {
                url: "http://localhost:8443".to_string(),
                allow_offline: true,
                offline_dirs: vec![],
                auth_token: None,
                tls: ModuleRegistryTlsSection::default(),
            },
            storage: ModuleStorageSection {
                install_dir: "tmp/test-modules".to_string(),
                cache_dir: Some("tmp/test-modules/cache".to_string()),
            },
            runtime: ModuleRuntimeSection::default(),
            bootstrap: Vec::new(),
            trust: ModuleTrustSection::default(),
            dev_sources: ModuleDevSourcesSection::default(),
            services: HashMap::new(),
        },
    })
}

fn test_dependencies() -> CliDependencies {
    let config = test_config();
    let registry = Arc::new(ServiceRegistry::new());
    let scheduler = Arc::new(SchedulerService::new(Arc::clone(&registry)));

    let mut adapters: BTreeMap<DbEngine, Arc<dyn DbAdminPort>> = BTreeMap::new();
    adapters.insert(DbEngine::Postgres, Arc::new(DummyDbAdapter));
    let db_shell = Arc::new(DbShellService::new(DbEngine::Postgres, adapters).expect("db shell"));

    let audit = Arc::new(InMemoryAuditLog::new(32));

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell),
        Arc::clone(&scheduler),
        Arc::clone(&registry),
        audit,
    ));

    CliDependencies::new(config, services)
}

#[test]
fn parse_state_after_command_space_identifies_subcommand_slot() {
    let line = "search ";
    let state = parse_state(line, line.len());

    assert_eq!(state.tokens, vec!["search"]);
    assert_eq!(state.prefix, "");
    assert_eq!(state.active_index, 1);
}

#[test]
fn parse_state_without_trailing_space_keeps_prefix() {
    let line = "search mo";
    let state = parse_state(line, line.len());

    assert_eq!(state.tokens, vec!["search"]);
    assert_eq!(state.prefix, "mo");
    assert_eq!(state.token_start, line.len() - 2);
}

#[test]
fn suggestions_for_search_command_stays_at_root_until_space() {
    let dependencies = test_dependencies();
    let registry = builtins::build_registry();
    let shapes = registry.shapes();
    let completer = ContextualCompleter::new(shapes, dependencies, ShellEnvironment::Cli);

    let (_, suggestions) = completer.suggestions_for("search", "search".len());
    assert_eq!(suggestions, vec!["search".to_string()]);
}

#[test]
fn search_command_suggests_module_resource() {
    let dependencies = test_dependencies();
    let registry = builtins::build_registry();
    let shapes = registry.shapes();
    let completer = ContextualCompleter::new(shapes, dependencies, ShellEnvironment::Cli);

    let (_, suggestions) = completer.suggestions_for("search ", "search ".len());
    assert_eq!(
        suggestions,
        vec!["module".to_string(), "modules".to_string()]
    );
}

#[test]
fn cycles_command_alias_before_subcommands() {
    let dependencies = test_dependencies();
    let registry = builtins::build_registry();
    let shapes = registry.shapes();
    let completer = ContextualCompleter::new(shapes, dependencies, ShellEnvironment::Cli);

    let (_, suggestions) = completer.suggestions_for("import", "import".len());
    assert_eq!(suggestions, vec!["import".to_string()]);

    let first_cycle = completer
        .cycle_suggestions("import", "import".len())
        .1
        .first()
        .map(|s| s.to_string())
        .expect("first cycle suggestion");
    assert_eq!(first_cycle, "import");

    let (_, canonical) = completer.suggestions_for("inst", "inst".len());
    assert!(
        canonical.contains(&"install".to_string()),
        "canonical command must stay discoverable via its own prefix"
    );
}

#[test]
fn cycles_short_prefix_through_commands_before_subcommands() {
    let dependencies = test_dependencies();
    let registry = builtins::build_registry();
    let shapes = registry.shapes();
    let completer = ContextualCompleter::new(shapes, dependencies, ShellEnvironment::Cli);

    let line = "s";
    let pos = line.len();
    let (_, suggestions) = completer.suggestions_for(line, pos);
    assert_eq!(
        suggestions.first().map(|s| s.as_str()),
        Some("search"),
        "expected prefix cycling to prioritise the primary match",
    );
    assert!(suggestions.contains(&"start".to_string()));
    assert!(suggestions.contains(&"stop".to_string()));

    let cycle_results = completer.cycle_suggestions(line, pos);
    let first_cycle = cycle_results.1.first().expect("cycle suggestion");
    assert_eq!(first_cycle, "search");
}
