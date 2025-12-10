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
