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
        "assets/logo/athenos.svg",
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
    let module_id = ModuleId::new("athenos-web").unwrap();
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
    let module_id = ModuleId::new("athenos-web").unwrap();
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

// ---------------------------------------------------------------------------
// Static `/gateway` proxy: streaming, disconnects, header rules
// ---------------------------------------------------------------------------

mod gateway_proxy {
    use super::*;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;
    use tokio::sync::{mpsc, Notify};
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);
    const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\n\r\n";

    fn chunk(data: &str) -> String {
        format!("{:x}\r\n{data}\r\n", data.len())
    }

    async fn read_head(socket: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match socket.read(&mut byte).await {
                Ok(1) => head.push(byte[0]),
                _ => break,
            }
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    /// Resolves once the peer has closed the connection.
    async fn wait_for_close(socket: &mut TcpStream) {
        let mut buf = [0_u8; 1024];
        while matches!(socket.read(&mut buf).await, Ok(n) if n > 0) {}
    }

    /// A control-plane stand-in speaking raw HTTP/1.1, so a test decides
    /// every byte of the response. Each request head goes to the receiver.
    async fn fake_upstream<F, Fut>(script: F) -> (String, mpsc::UnboundedReceiver<String>)
    where
        F: Fn(TcpStream, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (heads_tx, heads_rx) = mpsc::unbounded_channel();
        let script = Arc::new(script);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let script = Arc::clone(&script);
                let heads_tx = heads_tx.clone();
                tokio::spawn(async move {
                    let head = read_head(&mut socket).await;
                    let _ = heads_tx.send(head.clone());
                    script(socket, head).await;
                });
            }
        });
        (format!("http://{addr}"), heads_rx)
    }

    /// The static router on a real socket, proxying `/gateway` to `upstream`.
    async fn static_proxy(upstream: &str, first_byte: Duration, idle: Duration) -> String {
        let dir = std::env::temp_dir().join(format!("fenrir-proxy-{}", Uuid::new_v4()));
        let profile = StaticSiteProfile::new(dir.join("browser"), dir.join("browser/index.html"));
        let mut state = StaticRouterState::new(
            &ModuleId::new("athenos-web").unwrap(),
            profile,
            Arc::new(ServiceDiagnostics::new()),
            upstream.to_string(),
            dir.join("web.log"),
        );
        state.proxy.first_byte_timeout = first_byte;
        state.proxy.idle_timeout = idle;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                static_router(state).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        format!("http://{addr}")
    }

    async fn default_proxy(upstream: &str) -> String {
        static_proxy(
            upstream,
            STATIC_PROXY_FIRST_BYTE_TIMEOUT,
            STATIC_PROXY_IDLE_TIMEOUT,
        )
        .await
    }

    /// The browser. Its total timeout only turns a hang into a failure.
    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(2 * WAIT)
            .build()
            .unwrap()
    }

    /// Raw HTTP/1.1 exchange, so the test controls (and sees) every header.
    async fn raw_exchange(addr: &str, request: &str) -> (Vec<(String, String)>, Vec<u8>) {
        let mut socket = TcpStream::connect(addr.trim_start_matches("http://"))
            .await
            .unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let head = timeout(WAIT, read_head(&mut socket)).await.unwrap();
        let headers: Vec<(String, String)> = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .and_then(|(_, value)| value.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0_u8; length];
        timeout(WAIT, socket.read_exact(&mut body))
            .await
            .unwrap()
            .unwrap();
        (headers, body)
    }

    fn values<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
        headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[tokio::test]
    async fn event_stream_arrives_incrementally_before_upstream_closes() {
        let release = Arc::new(Notify::new());
        let upstream_done = Arc::new(AtomicBool::new(false));
        let (upstream, _heads) = fake_upstream({
            let release = Arc::clone(&release);
            let upstream_done = Arc::clone(&upstream_done);
            move |mut socket, _head| {
                let release = Arc::clone(&release);
                let upstream_done = Arc::clone(&upstream_done);
                async move {
                    let first = chunk("event: maintenance\ndata: {\"active\":false}\n\n");
                    socket
                        .write_all(format!("{SSE_HEAD}{first}").as_bytes())
                        .await
                        .unwrap();
                    release.notified().await;
                    let rest = format!(
                        "{}{}0\r\n\r\n",
                        chunk(": keep-alive\n\n"),
                        chunk("event: maintenance\ndata: {\"active\":true}\n\n")
                    );
                    socket.write_all(rest.as_bytes()).await.unwrap();
                    upstream_done.store(true, Ordering::SeqCst);
                }
            }
        })
        .await;
        let proxy = default_proxy(&upstream).await;

        let response = timeout(
            WAIT,
            client()
                .get(format!(
                    "{proxy}/gateway/services/x/api/v1/maintenance/stream"
                ))
                .header("accept", "text/event-stream")
                .send(),
        )
        .await
        .expect("response head arrives while the stream is open")
        .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap().to_string())
        };
        assert_eq!(header("content-type").as_deref(), Some("text/event-stream"));
        assert_eq!(header("cache-control").as_deref(), Some("no-cache"));
        assert_eq!(header("content-encoding"), None);
        assert_eq!(header("content-length"), None);
        // proxied responses keep the upstream's headers only
        assert_eq!(header("x-content-type-options"), None);

        let mut body = response.bytes_stream();
        let first = timeout(WAIT, body.next())
            .await
            .expect("first event arrives before the upstream closes")
            .unwrap()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&first)
                .contains("event: maintenance\ndata: {\"active\":false}"),
            "{first:?}"
        );
        assert!(!upstream_done.load(Ordering::SeqCst));

        release.notify_one();
        let mut rest = Vec::new();
        while let Some(part) = timeout(WAIT, body.next()).await.unwrap() {
            rest.extend_from_slice(&part.unwrap());
        }
        let rest = String::from_utf8(rest).unwrap();
        assert!(rest.contains(": keep-alive"), "{rest}");
        assert!(rest.contains("{\"active\":true}"), "{rest}");
        assert!(upstream_done.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn client_disconnect_drops_the_upstream_request() {
        // after the response head: the upstream is mid-stream
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<&'static str>();
        let (upstream, _heads) = fake_upstream(move |mut socket, head| {
            let closed_tx = closed_tx.clone();
            async move {
                if head.contains("/streaming") {
                    let first = chunk("event: feature-flags\ndata: []\n\n");
                    socket
                        .write_all(format!("{SSE_HEAD}{first}").as_bytes())
                        .await
                        .unwrap();
                    wait_for_close(&mut socket).await;
                    let _ = closed_tx.send("streaming");
                } else {
                    // before the response head: the upstream never answers
                    wait_for_close(&mut socket).await;
                    let _ = closed_tx.send("silent");
                }
            }
        })
        .await;
        let proxy = default_proxy(&upstream).await;

        let response = client()
            .get(format!("{proxy}/gateway/streaming"))
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        let mut body = response.bytes_stream();
        let first = timeout(WAIT, body.next()).await.unwrap().unwrap().unwrap();
        assert!(String::from_utf8_lossy(&first).contains("feature-flags"));
        drop(body); // the browser goes away mid-stream
        assert_eq!(
            timeout(WAIT, closed_rx.recv())
                .await
                .expect("upstream closed"),
            Some("streaming")
        );

        let gave_up = client()
            .get(format!("{proxy}/gateway/silent"))
            .timeout(Duration::from_millis(300))
            .send()
            .await;
        assert!(gave_up.is_err());
        assert_eq!(
            timeout(WAIT, closed_rx.recv())
                .await
                .expect("upstream closed"),
            Some("silent")
        );
    }

    /// The live path, both hops: browser → static edge (41002) → control
    /// plane (8080, its event-stream hop) → athenos-api. However the browser
    /// leaves, the stream upstream ends promptly, so athenos-api frees the
    /// stream slot it holds for it (SSE_LEAK).
    #[tokio::test]
    async fn a_stream_through_both_hops_ends_upstream_when_the_browser_leaves() {
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<()>();
        let (upstream, _heads) = fake_upstream(move |mut socket, _head| {
            let closed_tx = closed_tx.clone();
            async move {
                let first = chunk("event: maintenance\ndata: {\"active\":false}\n\n");
                socket
                    .write_all(format!("{SSE_HEAD}{first}").as_bytes())
                    .await
                    .unwrap();
                // silent from here on: only the proxy closing ends it
                wait_for_close(&mut socket).await;
                let _ = closed_tx.send(());
            }
        })
        .await;
        let hop = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hop_addr = hop.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(hop, crate::infra::http::event_stream_hop(upstream)).await;
        });
        let edge = default_proxy(&format!("http://{hop_addr}")).await;
        let edge = edge.trim_start_matches("http://").to_string();

        for (n, how) in ["close", "half-close", "reset"]
            .into_iter()
            .cycle()
            .take(30)
            .enumerate()
        {
            let mut socket = TcpStream::connect(&edge).await.unwrap();
            socket
                .write_all(
                    b"GET /gateway/services/x/api/v1/maintenance/stream HTTP/1.1\r\n\
                      host: athenos\r\naccept: text/event-stream\r\n\r\n",
                )
                .await
                .unwrap();
            let head = timeout(WAIT, read_head(&mut socket)).await.unwrap();
            assert!(head.contains("text/event-stream"), "{head}");
            let mut seen = Vec::new();
            let mut buf = [0_u8; 256];
            while !String::from_utf8_lossy(&seen).contains("maintenance") {
                let read = timeout(WAIT, socket.read(&mut buf)).await.unwrap().unwrap();
                assert!(read > 0, "the stream ended early");
                seen.extend_from_slice(&buf[..read]);
            }
            let _kept = match how {
                "close" => {
                    drop(socket);
                    None
                }
                "half-close" => {
                    socket.shutdown().await.unwrap();
                    Some(socket)
                }
                _ => {
                    socket.set_linger(Some(Duration::ZERO)).unwrap();
                    drop(socket);
                    None
                }
            };
            assert_eq!(
                timeout(Duration::from_secs(2), closed_rx.recv())
                    .await
                    .ok()
                    .flatten(),
                Some(()),
                "stream {n} ({how}): still open upstream after the browser left"
            );
        }
    }

    #[tokio::test]
    async fn keeps_every_set_cookie_and_drops_hop_by_hop_headers() {
        let (upstream, mut heads) = fake_upstream(|mut socket, _head| async move {
            let response = "HTTP/1.1 200 OK\r\n\
                content-type: text/plain\r\n\
                set-cookie: athenos_refresh=r1; Path=/; HttpOnly; SameSite=Strict\r\n\
                set-cookie: athenos_csrf=c1; Path=/\r\n\
                connection: keep-alive, x-upstream-hop\r\n\
                keep-alive: timeout=5\r\n\
                x-upstream-hop: 1\r\n\
                proxy-authenticate: Basic\r\n\
                trailer: x-checksum\r\n\
                upgrade: h2c\r\n\
                x-end-to-end: kept\r\n\
                content-length: 2\r\n\r\nok";
            socket.write_all(response.as_bytes()).await.unwrap();
            wait_for_close(&mut socket).await;
        })
        .await;
        let proxy = default_proxy(&upstream).await;

        let (headers, body) = raw_exchange(
            &proxy,
            "GET /gateway/services/x/api/v1/auth/refresh?probe=1 HTTP/1.1\r\n\
             host: 127.0.0.1\r\n\
             connection: keep-alive, x-client-hop\r\n\
             keep-alive: timeout=5\r\n\
             x-client-hop: 1\r\n\
             te: trailers\r\n\
             proxy-authorization: Basic eDp5\r\n\
             upgrade: websocket\r\n\
             cookie: athenos_refresh=r0\r\n\
             x-end-to-end: kept\r\n\r\n",
        )
        .await;
        assert_eq!(body, b"ok");
        assert_eq!(
            values(&headers, "set-cookie"),
            vec![
                "athenos_refresh=r1; Path=/; HttpOnly; SameSite=Strict",
                "athenos_csrf=c1; Path=/"
            ]
        );
        assert_eq!(values(&headers, "x-end-to-end"), vec!["kept"]);
        assert_eq!(values(&headers, "content-type"), vec!["text/plain"]);
        for hop in [
            "keep-alive",
            "x-upstream-hop",
            "proxy-authenticate",
            "trailer",
            "upgrade",
        ] {
            assert!(values(&headers, hop).is_empty(), "response leaked {hop}");
        }
        for value in values(&headers, "connection") {
            assert!(!value.contains("x-upstream-hop"), "{value}");
        }

        let upstream_head = timeout(WAIT, heads.recv()).await.unwrap().unwrap();
        let upstream_headers: Vec<(String, String)> = upstream_head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        assert!(
            upstream_head
                .starts_with("GET /gateway/services/x/api/v1/auth/refresh?probe=1 HTTP/1.1"),
            "{upstream_head}"
        );
        assert_eq!(
            values(&upstream_headers, "cookie"),
            vec!["athenos_refresh=r0"]
        );
        assert_eq!(values(&upstream_headers, "x-end-to-end"), vec!["kept"]);
        assert_eq!(
            values(&upstream_headers, "host"),
            vec![upstream.trim_start_matches("http://")]
        );
        for hop in [
            "connection",
            "keep-alive",
            "x-client-hop",
            "te",
            "proxy-authorization",
            "upgrade",
            "transfer-encoding",
            "content-length",
        ] {
            assert!(
                values(&upstream_headers, hop).is_empty(),
                "request leaked {hop}: {upstream_head}"
            );
        }
    }

    #[tokio::test]
    async fn large_bodies_stream_in_both_directions() {
        const PART: usize = 1 << 20;
        const TOTAL: usize = 4 * PART;
        let pattern = |i: usize| (i % 251) as u8;
        let release = Arc::new(Notify::new());
        let (half_tx, mut half_rx) = mpsc::unbounded_channel::<()>();
        let (upstream, mut heads) = fake_upstream({
            let release = Arc::clone(&release);
            move |mut socket, head| {
                let release = Arc::clone(&release);
                let half_tx = half_tx.clone();
                async move {
                    if head.starts_with("POST") {
                        // request body: the first half must arrive before the
                        // client has sent the second one
                        let mut half = vec![0_u8; 64 * 1024];
                        socket.read_exact(&mut half).await.unwrap();
                        assert!(half.iter().all(|b| *b == b'a'));
                        let _ = half_tx.send(());
                        socket.read_exact(&mut half).await.unwrap();
                        assert!(half.iter().all(|b| *b == b'b'));
                        socket
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                            .await
                            .unwrap();
                        wait_for_close(&mut socket).await;
                        return;
                    }
                    let data: Vec<u8> = (0..TOTAL).map(pattern).collect();
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {TOTAL}\r\nconnection: close\r\n\r\n"
                    );
                    socket.write_all(head.as_bytes()).await.unwrap();
                    socket.write_all(&data[..PART]).await.unwrap();
                    release.notified().await;
                    socket.write_all(&data[PART..]).await.unwrap();
                    wait_for_close(&mut socket).await;
                }
            }
        })
        .await;
        let proxy = default_proxy(&upstream).await;

        // response body
        let response = timeout(WAIT, client().get(format!("{proxy}/gateway/export")).send())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.headers().get("content-length").unwrap(),
            &TOTAL.to_string()
        );
        let mut body = response.bytes_stream();
        let mut received = Vec::with_capacity(TOTAL);
        while received.len() < PART {
            let part = timeout(WAIT, body.next())
                .await
                .expect("first megabyte arrives before the upstream sends the rest")
                .unwrap()
                .unwrap();
            received.extend_from_slice(&part);
        }
        release.notify_one();
        while let Some(part) = timeout(WAIT, body.next()).await.unwrap() {
            received.extend_from_slice(&part.unwrap());
        }
        assert_eq!(received.len(), TOTAL);
        assert!(received.iter().enumerate().all(|(i, b)| *b == pattern(i)));
        let _ = heads.recv().await;

        // request body, streamed with its content-length
        let (parts_tx, parts_rx) = mpsc::unbounded_channel::<Result<Vec<u8>, std::io::Error>>();
        parts_tx.send(Ok(vec![b'a'; 64 * 1024])).unwrap();
        let upload = tokio::spawn({
            let proxy = proxy.clone();
            async move {
                client()
                    .post(format!("{proxy}/gateway/upload"))
                    .header("content-length", (128 * 1024).to_string())
                    .body(reqwest::Body::wrap_stream(
                        tokio_stream::wrappers::UnboundedReceiverStream::new(parts_rx),
                    ))
                    .send()
                    .await
            }
        });
        timeout(WAIT, half_rx.recv())
            .await
            .expect("first half reaches the upstream while the client still sends");
        parts_tx.send(Ok(vec![b'b'; 64 * 1024])).unwrap();
        drop(parts_tx);
        let response = timeout(WAIT, upload).await.unwrap().unwrap().unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let upload_head = timeout(WAIT, heads.recv()).await.unwrap().unwrap();
        assert!(
            upload_head.contains("content-length: 131072"),
            "{upload_head}"
        );
        assert!(!upload_head.contains("transfer-encoding"), "{upload_head}");
    }

    #[tokio::test]
    async fn silent_streams_and_missing_heads_time_out() {
        let (closed_tx, mut closed_rx) = mpsc::unbounded_channel::<&'static str>();
        let (upstream, _heads) = fake_upstream(move |mut socket, head| {
            let closed_tx = closed_tx.clone();
            async move {
                if head.contains("/quiet-stream") {
                    let first = chunk("event: maintenance\ndata: {}\n\n");
                    socket
                        .write_all(format!("{SSE_HEAD}{first}").as_bytes())
                        .await
                        .unwrap();
                }
                wait_for_close(&mut socket).await;
                let _ = closed_tx.send(if head.contains("/quiet-stream") {
                    "stream"
                } else {
                    "head"
                });
            }
        })
        .await;
        let proxy = static_proxy(
            &upstream,
            Duration::from_millis(300),
            Duration::from_millis(300),
        )
        .await;

        let started = Instant::now();
        let response = client()
            .get(format!("{proxy}/gateway/no-head"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::GATEWAY_TIMEOUT);
        assert!(started.elapsed() < WAIT);
        assert_eq!(timeout(WAIT, closed_rx.recv()).await.unwrap(), Some("head"));

        let response = client()
            .get(format!("{proxy}/gateway/quiet-stream"))
            .header("accept", "text/event-stream")
            .send()
            .await
            .unwrap();
        let mut body = response.bytes_stream();
        let first = timeout(WAIT, body.next()).await.unwrap().unwrap().unwrap();
        assert!(String::from_utf8_lossy(&first).contains("event: maintenance"));
        // no heartbeat within the idle timeout: the stream is cut
        let cut = timeout(WAIT, body.next()).await.expect("cut after idle");
        assert!(!matches!(cut, Some(Ok(_))), "{cut:?}");
        assert_eq!(
            timeout(WAIT, closed_rx.recv()).await.unwrap(),
            Some("stream")
        );
    }

    // --- 5.6: client address at the edge, compression -------------------

    fn json_body(len: usize) -> Vec<u8> {
        let mut body = b"[".to_vec();
        let mut n = 0u64;
        while body.len() < len {
            n = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            body.extend_from_slice(
                format!(
                    "{{\"id\":\"{:016x}\",\"title\":\"Ticket {}\"}},",
                    n,
                    n % 9973
                )
                .as_bytes(),
            );
        }
        body.truncate(len);
        body
    }

    fn decode(encoding: &str, bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        match encoding {
            "br" => {
                brotli::Decompressor::new(bytes, 4096)
                    .read_to_end(&mut out)
                    .unwrap();
            }
            "gzip" => {
                flate2::read::GzDecoder::new(bytes)
                    .read_to_end(&mut out)
                    .unwrap();
            }
            other => panic!("unexpected encoding {other}"),
        }
        out
    }

    /// Everything a partial Brotli stream already decodes to.
    fn decode_prefix(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut reader = brotli::Decompressor::new(bytes, 4096);
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    fn header_values(response: &reqwest::Response, name: &str) -> Vec<String> {
        response
            .headers()
            .get_all(name)
            .iter()
            .map(|value| value.to_str().unwrap().to_string())
            .collect()
    }

    /// An upstream answering every request with `body` as `content_type`
    /// (plus `extra` header lines).
    async fn fixed_upstream(
        body: Vec<u8>,
        content_type: &'static str,
        extra: &'static str,
    ) -> (String, mpsc::UnboundedReceiver<String>) {
        let body = Arc::new(body);
        fake_upstream(move |mut socket, _head| {
            let body = Arc::clone(&body);
            async move {
                // One request per connection: the proxy must not reuse it.
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\n{extra}connection: close\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
                let _ = socket.shutdown().await;
            }
        })
        .await
    }

    #[tokio::test]
    async fn the_edge_replaces_client_address_headers_with_the_peer() {
        let (upstream, mut heads) = fixed_upstream(b"{}".to_vec(), "application/json", "").await;
        let proxy = default_proxy(&upstream).await;
        let (_headers, body) = raw_exchange(
            &proxy,
            "GET /gateway/services/x/api/v1/auth/login/request HTTP/1.1\r\n\
             host: 127.0.0.1\r\n\
             x-forwarded-for: 1.2.3.4\r\n\
             x-forwarded-for: 5.6.7.8\r\n\
             x-real-ip: 1.2.3.4\r\n\
             forwarded: for=1.2.3.4\r\n\
             x-fenrir-edge-hop: forged\r\n\
             accept-encoding: br\r\n\r\n",
        )
        .await;
        assert_eq!(body, b"{}");
        let head = timeout(WAIT, heads.recv()).await.unwrap().unwrap();
        let upstream_headers: Vec<(String, String)> = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        assert_eq!(
            values(&upstream_headers, "x-forwarded-for"),
            vec!["127.0.0.1"]
        );
        assert!(values(&upstream_headers, "x-real-ip").is_empty());
        assert!(values(&upstream_headers, "forwarded").is_empty());
        assert!(values(&upstream_headers, "accept-encoding").is_empty());
        assert_eq!(
            values(&upstream_headers, "x-fenrir-edge-hop"),
            vec![crate::infra::http::proxy_headers::edge_hop_secret()]
        );
    }

    #[tokio::test]
    async fn proxied_json_is_compressed_by_negotiation() {
        let original = json_body(40_000);
        let (upstream, _heads) =
            fixed_upstream(original.clone(), "application/json; charset=utf-8", "").await;
        let proxy = default_proxy(&upstream).await;
        let url = format!("{proxy}/gateway/services/x/api/v1/work/tickets");
        for (accept, expected) in [
            ("gzip, deflate, br, zstd", "br"),
            ("gzip", "gzip"),
            ("br;q=0.2, gzip;q=0.8", "gzip"),
        ] {
            let response = client()
                .get(&url)
                .header("accept-encoding", accept)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), 200);
            assert_eq!(header_values(&response, "content-encoding"), vec![expected]);
            assert_eq!(header_values(&response, "vary"), vec!["accept-encoding"]);
            assert!(response.headers().get("content-length").is_none());
            let packed = response.bytes().await.unwrap();
            assert!(
                packed.len() < original.len() / 3,
                "{accept}: {}",
                packed.len()
            );
            assert_eq!(decode(expected, &packed), original, "{accept}");
        }
        // No Accept-Encoding: identity, but still `Vary`.
        let response = client().get(&url).send().await.unwrap();
        assert!(response.headers().get("content-encoding").is_none());
        assert_eq!(header_values(&response, "vary"), vec!["accept-encoding"]);
        assert_eq!(
            header_values(&response, "content-length"),
            vec![original.len().to_string()]
        );
        assert_eq!(response.bytes().await.unwrap().to_vec(), original);
    }

    #[tokio::test]
    async fn secrets_cookies_small_and_encoded_bodies_stay_as_they_are() {
        let big = json_body(10_000);
        let cases: Vec<(Vec<u8>, &'static str, &'static str, &'static str)> = vec![
            // BREACH: token-bearing paths and Set-Cookie.
            (
                big.clone(),
                "application/json",
                "",
                "/api/v1/auth/login/confirm",
            ),
            (big.clone(), "application/json", "", "/api/v1/auth/refresh"),
            (
                big.clone(),
                "application/json",
                "",
                "/api/v1/admin/api-keys",
            ),
            (
                big.clone(),
                "application/json",
                "set-cookie: athenos_refresh=r1; HttpOnly\r\n",
                "/api/v1/work/tickets",
            ),
            // Small bodies and types that do not compress.
            (
                b"{\"ok\":true}".to_vec(),
                "application/json",
                "",
                "/api/v1/work/stats",
            ),
            (big.clone(), "image/png", "", "/api/v1/files/1"),
            (big.clone(), "font/woff2", "", "/api/v1/files/2"),
            // Already encoded upstream: passed on untouched.
            (
                big.clone(),
                "application/json",
                "content-encoding: gzip\r\n",
                "/api/v1/work/x",
            ),
        ];
        for (body, content_type, extra, path) in cases {
            let (upstream, _heads) = fixed_upstream(body.clone(), content_type, extra).await;
            let proxy = default_proxy(&upstream).await;
            let response = client()
                .get(format!("{proxy}/gateway/services/x{path}"))
                .header("accept-encoding", "gzip, br")
                .send()
                .await
                .unwrap();
            let encoding = header_values(&response, "content-encoding");
            if extra.starts_with("content-encoding") {
                assert_eq!(encoding, vec!["gzip"], "{path}");
            } else {
                assert!(encoding.is_empty(), "{path} {content_type} was compressed");
            }
            assert_eq!(response.bytes().await.unwrap().to_vec(), body, "{path}");
        }
    }

    #[tokio::test]
    async fn event_streams_stay_uncompressed_and_live() {
        let release = Arc::new(Notify::new());
        let gate = Arc::clone(&release);
        let (upstream, _heads) = fake_upstream(move |mut socket, _head| {
            let gate = Arc::clone(&gate);
            async move {
                let first = chunk("event: maintenance\ndata: {\"active\":false}\n\n");
                socket
                    .write_all(format!("{SSE_HEAD}{first}").as_bytes())
                    .await
                    .unwrap();
                gate.notified().await;
                let _ = socket.write_all(b"0\r\n\r\n").await;
            }
        })
        .await;
        let proxy = default_proxy(&upstream).await;
        let response = timeout(
            WAIT,
            client()
                .get(format!(
                    "{proxy}/gateway/services/x/api/v1/maintenance/stream"
                ))
                .header("accept", "text/event-stream")
                .header("accept-encoding", "gzip, deflate, br")
                .send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.headers().get("content-encoding").is_none());
        assert_eq!(
            header_values(&response, "content-type"),
            vec!["text/event-stream"]
        );
        let mut stream = response.bytes_stream();
        let first = timeout(WAIT, stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("event: maintenance"));
        release.notify_one();
    }

    #[tokio::test]
    async fn compressed_bodies_keep_streaming() {
        const PART: usize = 512 * 1024;
        let original = json_body(2 * PART);
        let release = Arc::new(Notify::new());
        let gate = Arc::clone(&release);
        let body = Arc::new(original.clone());
        let (upstream, _heads) = fake_upstream(move |mut socket, _head| {
            let gate = Arc::clone(&gate);
            let body = Arc::clone(&body);
            async move {
                let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n";
                socket.write_all(head.as_bytes()).await.unwrap();
                socket
                    .write_all(format!("{:x}\r\n", PART).as_bytes())
                    .await
                    .unwrap();
                socket.write_all(&body[..PART]).await.unwrap();
                socket.write_all(b"\r\n").await.unwrap();
                gate.notified().await;
                socket
                    .write_all(format!("{:x}\r\n", PART).as_bytes())
                    .await
                    .unwrap();
                socket.write_all(&body[PART..]).await.unwrap();
                socket.write_all(b"\r\n0\r\n\r\n").await.unwrap();
                wait_for_close(&mut socket).await;
            }
        })
        .await;
        let proxy = default_proxy(&upstream).await;
        let response = client()
            .get(format!("{proxy}/gateway/services/x/api/v1/work/tickets"))
            .header("accept-encoding", "br")
            .send()
            .await
            .unwrap();
        assert_eq!(header_values(&response, "content-encoding"), vec!["br"]);
        let mut stream = response.bytes_stream();
        let mut received = Vec::new();
        // The first half arrives, compressed, while the upstream waits.
        while decode_prefix(&received).len() < PART {
            let chunk = timeout(WAIT, stream.next())
                .await
                .expect("compressed output before the upstream finished")
                .unwrap()
                .unwrap();
            received.extend_from_slice(&chunk);
        }
        assert_eq!(decode_prefix(&received)[..PART], original[..PART]);
        release.notify_one();
        while let Some(chunk) = timeout(WAIT, stream.next()).await.unwrap() {
            received.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(decode("br", &received), original);
    }
}

mod static_compression {
    use super::*;
    use tower::ServiceExt;

    fn site() -> (PathBuf, Router) {
        let dir = std::env::temp_dir().join(format!("fenrir-static-br-{}", Uuid::new_v4()));
        let browser = dir.join("browser");
        std::fs::create_dir_all(&browser).unwrap();
        let script: String = (0..4000)
            .map(|i| format!("export const v{i} = \"value-{}\";\n", i % 97))
            .collect();
        std::fs::write(browser.join("main-ABCD1234.js"), &script).unwrap();
        let html = format!(
            "<!doctype html><html><body>{}</body></html>",
            "<p>athenos</p>".repeat(200)
        );
        std::fs::write(browser.join("index.html"), html).unwrap();
        std::fs::write(browser.join("en.json"), "{\"a\":1}").unwrap();
        let png: Vec<u8> = (0..5000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        std::fs::write(browser.join("logo.png"), png).unwrap();
        let profile = StaticSiteProfile::new(browser.clone(), browser.join("index.html"));
        let mut profile = profile;
        profile.html_headers = vec![(
            HeaderName::from_static("content-security-policy-report-only"),
            HeaderValue::from_static(TEST_CSP),
        )];
        let state = StaticRouterState::new(
            &ModuleId::new("athenos-web").unwrap(),
            profile,
            Arc::new(ServiceDiagnostics::new()),
            "http://127.0.0.1:9".to_string(),
            dir.join("web.log"),
        );
        (browser, static_router(state))
    }

    async fn get(
        router: &Router,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::builder().uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }

    fn h<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn unbrotli(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        brotli::Decompressor::new(bytes, 4096)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    fn ungzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[tokio::test]
    async fn static_files_are_served_as_brotli_or_gzip() {
        let (root, router) = site();
        let original = std::fs::read(root.join("main-ABCD1234.js")).unwrap();
        let (status, plain, body) = get(&router, "/main-ABCD1234.js", &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, original);
        assert!(plain.get("content-encoding").is_none());
        assert_eq!(h(&plain, "vary"), Some("accept-encoding"));
        let etag = h(&plain, "etag").unwrap().to_string();

        let (_, br, body) = get(
            &router,
            "/main-ABCD1234.js",
            &[("accept-encoding", "gzip, deflate, br, zstd")],
        )
        .await;
        assert_eq!(h(&br, "content-encoding"), Some("br"));
        assert_eq!(h(&br, "vary"), Some("accept-encoding"));
        assert_eq!(
            h(&br, "etag"),
            Some(format!("{}-br\"", etag.trim_end_matches('"')).as_str())
        );
        assert_eq!(
            h(&br, "content-length"),
            Some(body.len().to_string().as_str())
        );
        assert!(body.len() < original.len() / 4);
        assert_eq!(unbrotli(&body), original);
        // H1 and H2 unchanged on the compressed variant.
        assert_eq!(h(&br, "cache-control"), Some(STATIC_CACHE_IMMUTABLE));
        assert_eq!(
            h(&br, "content-type"),
            Some("application/javascript; charset=utf-8")
        );
        for (name, value) in STATIC_DEFAULT_HEADERS {
            assert_eq!(h(&br, name), Some(*value), "{name}");
        }

        let (_, gz, body) = get(&router, "/main-ABCD1234.js", &[("accept-encoding", "gzip")]).await;
        assert_eq!(h(&gz, "content-encoding"), Some("gzip"));
        assert_eq!(ungzip(&body), original);

        // HTML keeps the module's headers (CSP) when compressed.
        let (_, html, body) = get(&router, "/app/inbox", &[("accept-encoding", "br")]).await;
        assert_eq!(h(&html, "content-encoding"), Some("br"));
        assert_eq!(
            h(&html, "content-security-policy-report-only"),
            Some(TEST_CSP)
        );
        assert_eq!(h(&html, "cache-control"), Some(STATIC_CACHE_REVALIDATE));
        assert!(String::from_utf8(unbrotli(&body))
            .unwrap()
            .starts_with("<!doctype html>"));
    }

    #[tokio::test]
    async fn every_variant_etag_revalidates_with_304() {
        let (_, router) = site();
        let (_, plain, _) = get(&router, "/main-ABCD1234.js", &[]).await;
        let etag = h(&plain, "etag").unwrap().to_string();
        let (_, br, _) = get(&router, "/main-ABCD1234.js", &[("accept-encoding", "br")]).await;
        let br_etag = h(&br, "etag").unwrap().to_string();
        assert_ne!(etag, br_etag);
        for (accept, presented) in [
            ("br", br_etag.clone()),
            ("br", etag.clone()),
            ("br", format!("W/{br_etag}")),
            ("", br_etag.clone()),
            ("gzip", format!("\"other\", {br_etag}")),
        ] {
            let mut headers = vec![("if-none-match", presented.as_str())];
            if !accept.is_empty() {
                headers.push(("accept-encoding", accept));
            }
            let (status, response, body) = get(&router, "/main-ABCD1234.js", &headers).await;
            assert_eq!(status, StatusCode::NOT_MODIFIED, "{accept} {presented}");
            assert!(body.is_empty());
            assert_eq!(h(&response, "vary"), Some("accept-encoding"));
            assert!(response.get("content-encoding").is_none());
        }
        let (status, _, _) = get(
            &router,
            "/main-ABCD1234.js",
            &[("if-none-match", "\"0000\""), ("accept-encoding", "br")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn small_and_binary_files_are_not_compressed() {
        let (root, router) = site();
        let (_, json, body) = get(&router, "/en.json", &[("accept-encoding", "br")]).await;
        assert!(json.get("content-encoding").is_none());
        assert_eq!(body, b"{\"a\":1}");
        let (_, png, body) = get(&router, "/logo.png", &[("accept-encoding", "br")]).await;
        assert!(png.get("content-encoding").is_none());
        assert!(png.get("vary").is_none());
        assert_eq!(body, std::fs::read(root.join("logo.png")).unwrap());
    }
}

mod access_log {
    //! 5.6, after 3.8a: invite tokens never reach the static site's log.
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn invite_tokens_are_masked_in_the_static_log() {
        let dir = std::env::temp_dir().join(format!("fenrir-static-log-{}", Uuid::new_v4()));
        let browser = dir.join("browser");
        std::fs::create_dir_all(&browser).unwrap();
        std::fs::write(browser.join("index.html"), "<!doctype html>").unwrap();
        let log = dir.join("web.log");
        let state = StaticRouterState::new(
            &ModuleId::new("athenos-web").unwrap(),
            StaticSiteProfile::new(browser.clone(), browser.join("index.html")),
            Arc::new(ServiceDiagnostics::new()),
            // Closed port: the gateway proxy fails and logs that too.
            "http://127.0.0.1:9".to_string(),
            log.clone(),
        );
        let router = static_router(state);
        for path in [
            "/invite/SECRET-INVITE-ONE",
            "/gateway/services/module%3Aathenos-api%3A%3Aapi-gateway/api/v1/invite/SECRET-INVITE-TWO/join?token=SECRET-QUERY-THREE",
            "/gateway/services/x/api/v1/workspaces/w1/invites/SECRET-INVITE-FOUR/revoke",
        ] {
            let response = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;
        }
        let written = std::fs::read_to_string(&log).unwrap();
        assert!(!written.contains("SECRET-"), "token in the log:\n{written}");
        assert!(written.contains("/invite/<redacted>"), "{written}");
        assert!(written.contains("/invites/<redacted>/revoke"), "{written}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
