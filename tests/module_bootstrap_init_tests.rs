use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

#[test]
fn bootstrap_init_writes_runtime_metadata() {
    let temp = TempDir::new().expect("tempdir");
    let module_root = temp.path().join("module-demo");
    fs::create_dir_all(&module_root).expect("module root created");

    let snapshot_path = temp.path().join("services.json");
    let snapshot_contents = serde_json::json!({
        "services": [
            {
                "uri": "service://module:demo::api",
                "endpoint": "http://127.0.0.1:9000/"
            }
        ]
    });
    fs::write(&snapshot_path, snapshot_contents.to_string()).expect("snapshot written");

    let mut cmd = Command::cargo_bin("fenrir-module-kit").expect("binary built");
    cmd.arg("init")
        .arg("--module-root")
        .arg(&module_root)
        .env("FENRIR_MODULE_ID", "demo")
        .env("FENRIR_SERVICE_ID", "module:demo::api")
        .env("FENRIR_SERVICE_URI", "service://module:demo::api")
        .env("FENRIR_SERVICE_TOKEN", "secret-token")
        .env("FENRIR_SERVICE_TOKEN_ISSUED_AT", "2024-01-01T00:00:00Z")
        .env("FENRIR_SERVICE_TOKEN_EXPIRES_AT", "2024-01-01T01:00:00Z")
        .env("FENRIR_SERVICE_TOKEN_TTL_SECS", "3600")
        .env("FENRIR_SERVICE_SNAPSHOT_PATH", &snapshot_path);
    cmd.assert().success();

    let runtime_file = module_root.join(".fenrir/runtime.json");
    assert!(runtime_file.exists(), "runtime metadata file created");
    let data = fs::read_to_string(&runtime_file).expect("runtime file readable");
    let json: Value = serde_json::from_str(&data).expect("valid json");

    assert_eq!(json["module"]["id"], "demo");
    assert_eq!(json["module"]["service_id"], "module:demo::api");
    assert_eq!(
        json["service_snapshot"]["services"][0]["uri"],
        "service://module:demo::api"
    );
    assert_eq!(
        json["token"]["issued_at"],
        Value::String("2024-01-01T00:00:00Z".to_string())
    );
    assert_eq!(json["token"]["ttl_secs"], Value::Number(3600u64.into()));
    assert!(json["generated_at"].as_str().is_some(), "generated_at set");
}
