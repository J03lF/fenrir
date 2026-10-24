use super::*;
use crate::domain::module::{
    ChecksumAlgorithm, ModuleBundle, ModuleInstallResult, ModuleInstallSource,
};
use crate::domain::module::{
    InstalledModule, ModuleArtifactDescriptor, ModuleChecksum, ModuleManifest,
    ModuleSignatureDescriptor, SignatureAlgorithm,
};
use crate::domain::module::{ModuleStorageError, ModuleStoragePort};
use crate::services::ServiceDiagnostics;
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
        if self.installed.manifest.id == id.as_str() {
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
            dependencies: vec![],
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
    let diagnostics = Arc::new(ServiceDiagnostics::new());
    let runtime = ProcessModuleRuntime::new(
        Arc::clone(&storage),
        tmp_dir.clone(),
        diagnostics,
        "http://127.0.0.1:4100".to_string(),
    );

    let state = PersistedModuleState {
        instance_id: "fenrir-api:primary".into(),
        primary: true,
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

// ---------------------------------------------------------------------------
// Static-site response headers and caching (H1/H2)
// ---------------------------------------------------------------------------

const TEST_CSP: &str = "default-src 'self'; frame-ancestors 'none'";

fn html_headers() -> Vec<(HeaderName, HeaderValue)> {
    vec![
        (
            HeaderName::from_static("content-security-policy-report-only"),
            HeaderValue::from_static(TEST_CSP),
        ),
        (
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("SAMEORIGIN"),
        ),
    ]
}

fn header_str<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

fn assert_static_defaults(response: &Response, label: &str) {
    for (name, value) in STATIC_DEFAULT_HEADERS {
        if *name == "x-frame-options" && header_str(response, name) == Some("SAMEORIGIN") {
            continue; // overridden by the module on HTML in some tests
        }
        assert_eq!(header_str(response, name), Some(*value), "{label}: {name}");
    }
}

fn assert_no_static_headers(response: &Response, label: &str) {
    for (name, _) in STATIC_DEFAULT_HEADERS {
        assert!(
            response.headers().get(*name).is_none(),
            "{label}: leaked {name}"
        );
    }
    assert!(
        response
            .headers()
            .get("content-security-policy-report-only")
            .is_none(),
        "{label}: leaked CSP"
    );
}

#[test]
fn static_response_sets_security_defaults_on_every_file() {
    for path in [
        "index.html",
        "chunk-ABAH6MG5.js",
        "theme-boot.js",
        "assets/i18n/en.json",
        "favicon.ico",
        "media/geist-ABCDEF12.woff2",
    ] {
        let response = build_static_response(b"x".to_vec(), Path::new(path), None, &[], None);
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_static_defaults(&response, path);
    }
}

#[test]
fn module_headers_apply_to_html_only_and_override_defaults() {
    let headers = html_headers();
    let html = build_static_response(
        b"<html>".to_vec(),
        Path::new("index.html"),
        None,
        &headers,
        None,
    );
    assert_eq!(
        header_str(&html, "content-security-policy-report-only"),
        Some(TEST_CSP)
    );
    assert_eq!(header_str(&html, "x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(header_str(&html, "x-content-type-options"), Some("nosniff"));
    assert_eq!(html.headers().get_all("x-frame-options").iter().count(), 1);

    for path in [
        "main-B3ISQ3XF.js",
        "theme-boot.js",
        "assets/i18n/en.json",
        "styles-BAY5EODU.css",
    ] {
        let asset = build_static_response(b"x".to_vec(), Path::new(path), None, &headers, None);
        assert!(
            asset
                .headers()
                .get("content-security-policy-report-only")
                .is_none(),
            "{path}: CSP must stay on HTML"
        );
        assert_eq!(
            header_str(&asset, "x-frame-options"),
            Some("DENY"),
            "{path}"
        );
    }
}

#[test]
fn only_content_hashed_files_are_immutable() {
    let immutable = [
        "chunk-ABAH6MG5.js",
        "chunk-ESKICHHY.js",
        "main-B3ISQ3XF.js",
        "polyfills-FFHMD2TL.js",
        "styles-BAY5EODU.css",
        "media/geist-latin-4CLCTAJ7.woff2",
        "main.0123456789abcdef.js",
        "font.0123456789abcdef0123.woff2",
    ];
    for path in immutable {
        let response = build_static_response(b"x".to_vec(), Path::new(path), None, &[], None);
        assert_eq!(
            header_str(&response, "cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{path}"
        );
    }
    let revalidate = [
        "index.html",
        "theme-boot.js",
        "assets/i18n/en.json",
        "assets/i18n/de.json",
        "favicon.ico",
        "assets/logo/athene.svg",
        "icon-192x192.png",
        "chunk-ABAH6MG5.js.map",
        "chunk-abah6mg5.js",
        "chunk-ABAH6MG.js",
        "chunk-ABAH6MG55.js",
        "-ABAH6MG5.js",
        "ABAH6MG5",
        "page-ABAH6MG5.html",
        "main.0123456789abcde.js",
    ];
    for path in revalidate {
        let response = build_static_response(b"x".to_vec(), Path::new(path), None, &[], None);
        assert_eq!(
            header_str(&response, "cache-control"),
            Some("no-cache"),
            "{path}"
        );
    }
}

#[test]
fn static_response_sets_etag_last_modified_and_answers_304() {
    // RFC 9110 example date: Sun, 06 Nov 1994 08:49:37 GMT
    let modified = UNIX_EPOCH + Duration::from_secs(784_111_777);
    let first = build_static_response(
        b"{\"a\":1}".to_vec(),
        Path::new("assets/i18n/en.json"),
        Some(modified),
        &[],
        None,
    );
    let etag = header_str(&first, "etag").expect("etag").to_string();
    assert!(
        etag.starts_with('"') && etag.ends_with('"') && etag.len() == 34,
        "{etag}"
    );
    assert_eq!(
        header_str(&first, "last-modified"),
        Some("Sun, 06 Nov 1994 08:49:37 GMT")
    );

    let other = build_static_response(b"{\"a\":2}".to_vec(), Path::new("en.json"), None, &[], None);
    assert_ne!(header_str(&other, "etag"), Some(etag.as_str()));

    for if_none_match in [
        etag.clone(),
        format!("W/{etag}"),
        format!("\"nope\", {etag}"),
        "*".into(),
    ] {
        let value = HeaderValue::from_str(&if_none_match).unwrap();
        let response = build_static_response(
            b"{\"a\":1}".to_vec(),
            Path::new("assets/i18n/en.json"),
            Some(modified),
            &[],
            Some(&value),
        );
        assert_eq!(
            response.status(),
            StatusCode::NOT_MODIFIED,
            "{if_none_match}"
        );
        assert_eq!(header_str(&response, "etag"), Some(etag.as_str()));
        assert_eq!(header_str(&response, "cache-control"), Some("no-cache"));
        assert!(response.headers().get("content-type").is_none());
        assert_static_defaults(&response, "304");
    }

    let stale = HeaderValue::from_static("\"0000\"");
    let response = build_static_response(
        b"{\"a\":1}".to_vec(),
        Path::new("assets/i18n/en.json"),
        None,
        &[],
        Some(&stale),
    );
    assert_eq!(response.status(), StatusCode::OK);
}

fn static_site_fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fenrir-static-{}", Uuid::new_v4()));
    let browser = dir.join("dist/browser");
    std::fs::create_dir_all(browser.join("assets/i18n")).unwrap();
    std::fs::create_dir_all(dir.join(".fenrir")).unwrap();
    std::fs::write(
        browser.join("index.html"),
        "<!doctype html><app-root></app-root>",
    )
    .unwrap();
    std::fs::write(browser.join("chunk-ABAH6MG5.js"), "console.log(1)").unwrap();
    std::fs::write(browser.join("theme-boot.js"), "void 0").unwrap();
    std::fs::write(browser.join("assets/i18n/en.json"), "{}").unwrap();
    std::fs::write(
        dir.join(RUNTIME_MANIFEST_PATH),
        format!(
            r#"[runtime]
mode = "static_site"

[runtime.static_site]
asset_roots = ["dist/browser"]
entrypoint = "dist/browser/index.html"
index_file = "index.html"

[runtime.static_site.response_headers]
"Content-Security-Policy-Report-Only" = "{TEST_CSP}"
"Cache-Control" = "public, max-age=999"
"Content-Type" = "text/plain"
"bad header" = "x"
"#
        ),
    )
    .unwrap();
    dir
}

#[test]
fn manifest_response_headers_are_parsed_and_host_owned_ones_dropped() {
    let dir = static_site_fixture();
    let manifest: RuntimeManifest =
        toml::from_str(&std::fs::read_to_string(dir.join(RUNTIME_MANIFEST_PATH)).unwrap()).unwrap();
    let module_id = ModuleId::new("athene-web").unwrap();
    let profile = ProcessModuleRuntime::static_profile_from_manifest(&module_id, &dir, &manifest)
        .unwrap()
        .expect("static profile");
    let names: Vec<&str> = profile
        .html_headers
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(names, vec!["content-security-policy-report-only"]);
    assert_eq!(profile.html_headers[0].1, TEST_CSP);

    // discovered profiles (no manifest) only get the host defaults
    let discovered = ProcessModuleRuntime::discover_static_profile(&dir).expect("discovered");
    assert!(discovered.html_headers.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn static_router_sets_headers_on_static_routes_only() {
    use axum::body::to_bytes;
    use tower::ServiceExt;

    let dir = static_site_fixture();
    let manifest: RuntimeManifest =
        toml::from_str(&std::fs::read_to_string(dir.join(RUNTIME_MANIFEST_PATH)).unwrap()).unwrap();
    let module_id = ModuleId::new("athene-web").unwrap();
    let profile = ProcessModuleRuntime::static_profile_from_manifest(&module_id, &dir, &manifest)
        .unwrap()
        .unwrap();
    let storage: Arc<dyn ModuleStoragePort> = Arc::new(TestStorage {
        installed: test_manifest(),
    });
    let diagnostics = Arc::new(ServiceDiagnostics::new());
    // control plane on a closed port: the gateway proxy answers 502 itself
    let runtime = ProcessModuleRuntime::new(
        storage,
        dir.clone(),
        Arc::clone(&diagnostics),
        "http://127.0.0.1:9".to_string(),
    );
    let router = runtime.build_static_router(&module_id, profile, diagnostics, dir.join("web.log"));

    let send = |path: &str, if_none_match: Option<String>| {
        let mut request = Request::builder().uri(path);
        if let Some(value) = if_none_match {
            request = request.header(header::IF_NONE_MATCH, value);
        }
        router.clone().oneshot(request.body(Body::empty()).unwrap())
    };

    // HTML: document and SPA fallback
    for path in ["/", "/app/inbox", "/missing.js"] {
        let response = send(path, None).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_static_defaults(&response, path);
        assert_eq!(
            header_str(&response, "content-security-policy-report-only"),
            Some(TEST_CSP),
            "{path}"
        );
        assert_eq!(
            header_str(&response, "cache-control"),
            Some("no-cache"),
            "{path}"
        );
        assert_eq!(
            header_str(&response, "content-type"),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
    }

    let chunk = send("/chunk-ABAH6MG5.js", None).await.unwrap();
    assert_static_defaults(&chunk, "chunk");
    assert_eq!(
        header_str(&chunk, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(chunk
        .headers()
        .get("content-security-policy-report-only")
        .is_none());

    for path in ["/theme-boot.js", "/assets/i18n/en.json"] {
        let response = send(path, None).await.unwrap();
        assert_static_defaults(&response, path);
        assert_eq!(
            header_str(&response, "cache-control"),
            Some("no-cache"),
            "{path}"
        );
        assert!(response.headers().get("last-modified").is_some(), "{path}");
        let etag = header_str(&response, "etag").unwrap().to_string();
        let revalidated = send(path, Some(etag)).await.unwrap();
        assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED, "{path}");
        let body = to_bytes(revalidated.into_body(), usize::MAX).await.unwrap();
        assert!(body.is_empty());
    }

    // non-static routes stay untouched
    for path in [
        "/live",
        "/ready",
        "/healthz",
        "/.fenrir/services",
        "/gateway/services/x",
    ] {
        let response = send(path, None).await.unwrap();
        assert_no_static_headers(&response, path);
        assert!(response.headers().get("etag").is_none(), "{path}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}
