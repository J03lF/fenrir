use std::env;
use std::fs;
use std::sync::{Mutex, OnceLock};

use super::error::ConfigError;
use super::load;
use super::loading::{
    explicit_config_path, ENV_CONFIG_ENV, ENV_CONFIG_FILE, ENV_ENV, ENV_FILE_OVERRIDE,
};

fn test_mutex() -> &'static Mutex<()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(()))
}

#[test]
fn default_config_loads_and_validates() {
    let _lock = test_mutex().lock().unwrap();
    env::set_var(
        "FENRIR_DB_POSTGRES_URI",
        "postgresql://localhost:5432/fenrir",
    );
    env::set_var("FENRIR_HTTP_TOKEN_ADMIN", "admin-token-example");
    env::set_var("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token-example");
    env::set_var("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token-example");
    env::set_var("FENRIR_REGISTRY_TOKEN", "registry-token-example-123");
    env::remove_var(ENV_CONFIG_FILE);
    env::remove_var(ENV_CONFIG_ENV);
    env::remove_var(ENV_ENV);

    let cfg = load().expect("config should load");
    assert!(!cfg.app.name.is_empty());
    env::remove_var("FENRIR_DB_POSTGRES_URI");
    env::remove_var("FENRIR_HTTP_TOKEN_ADMIN");
    env::remove_var("FENRIR_HTTP_TOKEN_OPERATOR");
    env::remove_var("FENRIR_HTTP_TOKEN_VIEWER");
    env::remove_var("FENRIR_REGISTRY_TOKEN");
}

#[test]
fn config_env_requires_existing_profile_file() {
    let _lock = test_mutex().lock().unwrap();
    env::set_var(
        "FENRIR_DB_POSTGRES_URI",
        "postgresql://localhost:5432/fenrir",
    );
    env::set_var("FENRIR_HTTP_TOKEN_ADMIN", "admin-token-example");
    env::set_var("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token-example");
    env::set_var("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token-example");
    env::set_var("FENRIR_REGISTRY_TOKEN", "registry-token-example-123");
    env::set_var(ENV_CONFIG_ENV, "does-not-exist");
    env::remove_var(ENV_CONFIG_FILE);
    env::remove_var(ENV_ENV);

    let err = load().expect_err("profile should be missing");
    match err {
        ConfigError::MissingConfigFile { path } => {
            assert!(path.ends_with("config/does-not-exist.toml"));
        }
        other => panic!("expected MissingConfigFile, got {other:?}"),
    }
    env::remove_var("FENRIR_DB_POSTGRES_URI");
    env::remove_var("FENRIR_HTTP_TOKEN_ADMIN");
    env::remove_var("FENRIR_HTTP_TOKEN_OPERATOR");
    env::remove_var("FENRIR_HTTP_TOKEN_VIEWER");
    env::remove_var("FENRIR_REGISTRY_TOKEN");
    env::remove_var(ENV_CONFIG_ENV);
}

#[test]
fn explicit_config_file_must_exist() {
    let _lock = test_mutex().lock().unwrap();
    let missing = env::temp_dir().join("fenrir-test-missing-config.toml");
    if missing.exists() {
        std::fs::remove_file(&missing).ok();
    }
    env::set_var(ENV_CONFIG_FILE, missing.to_string_lossy().to_string());
    env::remove_var(ENV_CONFIG_ENV);
    env::remove_var(ENV_ENV);

    let err = explicit_config_path().expect_err("should fail for missing file");
    match err {
        ConfigError::MissingConfigFile { .. } => {}
        other => panic!("expected MissingConfigFile, got {other:?}"),
    }
    env::remove_var(ENV_CONFIG_FILE);
}

#[test]
fn env_file_override_populates_missing_variables() {
    let _lock = test_mutex().lock().unwrap();
    for key in [
        "FENRIR_DB_POSTGRES_URI",
        "FENRIR_HTTP_TOKEN_ADMIN",
        "FENRIR_HTTP_TOKEN_OPERATOR",
        "FENRIR_HTTP_TOKEN_VIEWER",
        "FENRIR_REGISTRY_TOKEN",
    ] {
        env::remove_var(key);
    }
    env::remove_var(ENV_CONFIG_FILE);
    env::remove_var(ENV_CONFIG_ENV);
    env::remove_var(ENV_ENV);

    let env_path = env::temp_dir().join(format!(
        "fenrir-env-test-{}-{}.env",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let contents = r#"
FENRIR_DB_POSTGRES_URI=postgresql://localhost:5432/fenrir
export FENRIR_HTTP_TOKEN_ADMIN=admin-token-example
FENRIR_HTTP_TOKEN_OPERATOR=operator-token-example
FENRIR_HTTP_TOKEN_VIEWER=viewer-token-example
FENRIR_REGISTRY_TOKEN=registry-token-example-123
"#;
    fs::write(&env_path, contents).expect("writing env file should succeed");
    env::set_var(ENV_FILE_OVERRIDE, env_path.to_string_lossy().to_string());

    let cfg = load().expect("config should load with env file");
    assert!(!cfg.app.name.is_empty());

    fs::remove_file(&env_path).ok();
    env::remove_var(ENV_FILE_OVERRIDE);
    for key in [
        "FENRIR_DB_POSTGRES_URI",
        "FENRIR_HTTP_TOKEN_ADMIN",
        "FENRIR_HTTP_TOKEN_OPERATOR",
        "FENRIR_HTTP_TOKEN_VIEWER",
        "FENRIR_REGISTRY_TOKEN",
    ] {
        env::remove_var(key);
    }
}

const BASE_SECRETS: &[(&str, &str)] = &[
    (
        "FENRIR_DB_POSTGRES_URI",
        "postgresql://localhost:5432/fenrir",
    ),
    ("FENRIR_HTTP_TOKEN_ADMIN", "admin-token-example"),
    ("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token-example"),
    ("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token-example"),
    ("FENRIR_REGISTRY_TOKEN", "registry-token-example-123"),
];

/// Runs `body` with the base secrets and `extra` set, then removes them.
fn with_env<T>(extra: &[(&str, &str)], body: impl FnOnce() -> T) -> T {
    env::remove_var(ENV_CONFIG_FILE);
    env::remove_var(ENV_CONFIG_ENV);
    for (key, value) in BASE_SECRETS.iter().chain(extra) {
        env::set_var(key, value);
    }
    let result = body();
    for (key, _) in BASE_SECRETS.iter().chain(extra) {
        env::remove_var(key);
    }
    result
}

#[test]
fn edge_settings_come_from_the_documented_env_names() {
    let _lock = test_mutex().lock().unwrap();
    let cfg = with_env(
        &[
            ("FENRIR_SERVER__EDGE__CLIENT_IP_HEADER", "X-Real-IP"),
            (
                "FENRIR_SERVER__EDGE__TRUSTED_PROXIES",
                "127.0.0.1/32, ::1/128",
            ),
        ],
        || load().expect("edge env should load"),
    );
    let policy = cfg.server.edge.policy().unwrap();
    assert_eq!(policy.header(), Some("x-real-ip"));
    assert_eq!(policy.trusted_proxies().len(), 2);
    assert!(policy.is_trusted_peer("::1".parse().unwrap()));
}

#[test]
fn edge_settings_also_work_through_the_generic_env_source() {
    let _lock = test_mutex().lock().unwrap();
    let cfg = with_env(
        &[
            ("FENRIR__SERVER__EDGE__CLIENT_IP_HEADER", "cf-connecting-ip"),
            (
                "FENRIR__SERVER__EDGE__TRUSTED_PROXIES",
                "127.0.0.1,10.0.0.0/8",
            ),
        ],
        || load().expect("generic edge env should load"),
    );
    let policy = cfg.server.edge.policy().unwrap();
    assert_eq!(policy.header(), Some("cf-connecting-ip"));
    assert!(policy.is_trusted_peer("10.1.2.3".parse().unwrap()));
}

#[test]
fn edge_garbage_fails_at_load() {
    let _lock = test_mutex().lock().unwrap();
    let err = with_env(
        &[
            ("FENRIR_SERVER__EDGE__CLIENT_IP_HEADER", "x-real-ip"),
            (
                "FENRIR_SERVER__EDGE__TRUSTED_PROXIES",
                "127.0.0.1/32,banana",
            ),
        ],
        || load().expect_err("garbage CIDR must fail"),
    );
    assert!(err.to_string().contains("banana"), "{err}");
    let err = with_env(
        &[("FENRIR_SERVER__EDGE__CLIENT_IP_HEADER", "x-real-ip")],
        || load().expect_err("a header without proxies must fail"),
    );
    assert!(err.to_string().contains("trusted_proxies"), "{err}");
}

#[test]
fn edge_is_off_by_default() {
    let _lock = test_mutex().lock().unwrap();
    let cfg = with_env(&[], || load().expect("config should load"));
    assert_eq!(cfg.server.edge.policy().unwrap().header(), None);
}
