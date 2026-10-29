//! The host against a real Fenrir Depot, end to end, in the production
//! shape:
//!
//! ```text
//! host pipeline ── DepotHttpClient, mTLS ──▶ mtls_proxy ──Unix socket──▶ depot-server ──▶ Postgres
//!  (services::depot)   (client certificate      (what Caddy does:         (socket-only      (throwaway)
//!                       from `depot cert`)       require_and_verify,       identity)
//!                                                sets the fingerprint)
//! ```
//!
//! Two (then three) distributions are published with the real `depot` CLI
//! and signed with a throwaway release key; the host installs, checks for
//! newer, installs a module as a hotfix, pins, upgrades, rolls back and
//! meets a yank. Bundles are unpacked by the real `FilesystemModuleStorage`
//! into a temp modules directory. Process start, migrations and health of
//! the module runtime are a stand-in (`StorageHost`): the pipeline is
//! tested up to "verified bundle staged and the runtime called".
//!
//! Needs the depot binaries and a throwaway Postgres; `scripts/depot-e2e.sh`
//! provides both. Without `FENRIR_DEPOT_E2E_BIN` and
//! `DEPOT_TEST_DATABASE_URL` it prints SKIPPED.
#![cfg(feature = "depot")]

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use depot_protocol::ModuleVersion as DepotVersion;
use fenrir::config::ModuleStorageSection;
use fenrir::domain::module::{
    ModuleBundle, ModuleId, ModuleInstallSource, ModuleManifest, ModuleStoragePort,
};
use fenrir::infra::db::migrations::{list_sql_files_in, parse_migrations_dir};
use fenrir::infra::modules::depot::{DepotHttpClient, DepotSettings};
use fenrir::infra::modules::FilesystemModuleStorage;
use fenrir::services::depot::plan::Action;
use fenrir::services::depot::verify::newer_distribution;
use fenrir::services::depot::{
    execute, prepare_distribution, prepare_module, prepare_rollback, update_state, Depot,
    DepotError, DepotSource, DistributionRequest, HostModule, Intent, ModuleHost, StateStore,
};
use flate2::write::GzEncoder;
use flate2::Compression;

fn quiet(_: String) {}

// ─── the depot side ──────────────────────────────────────────────────────

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct DepotWorld {
    bin: PathBuf,
    work: PathBuf,
    url: String,
    _server: Killed,
    _proxy: Killed,
}

fn run(bin: &Path, args: &[&str]) -> String {
    let output = Command::new(bin)
        .args(args)
        .env_remove("DEPOT_URL")
        .output()
        .unwrap_or_else(|err| panic!("{} {args:?}: {err}", bin.display()));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "{} {args:?} failed:\n{stdout}\n{}",
        bin.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn private_file(path: &Path, text: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// The `depot-server client add …` line `depot cert issue` printed.
fn server_command(printed: &str) -> Vec<String> {
    let line = printed
        .lines()
        .find_map(|l| l.trim().strip_prefix("sudo -u depot depot-server "))
        .unwrap_or_else(|| panic!("no server command in:\n{printed}"));
    line.split_whitespace().map(str::to_string).collect()
}

impl DepotWorld {
    fn depot(&self, args: &[&str]) -> String {
        let w = &self.work;
        let joel = w.join("certs/joel");
        let mut argv = vec![
            "--server".to_string(),
            self.url.clone(),
            "--ca-cert".to_string(),
            w.join("ca/ca.pem").display().to_string(),
            "--client-cert".to_string(),
            joel.join("joel.pem").display().to_string(),
            "--client-key".to_string(),
            joel.join("joel.key").display().to_string(),
            "--key".to_string(),
            w.join("keys/release.key").display().to_string(),
        ];
        argv.extend(args.iter().map(|a| (*a).to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        run(&self.bin.join("depot"), &argv)
    }

    /// Bundles + sidecars + rules, then `depot publish … --yes`.
    fn publish(&self, product: &str, version: &str, modules: &[(&str, &str)]) {
        let dir = self
            .work
            .join("releases")
            .join(format!("{product}-{version}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rules = format!("product = \"{product}\"\n[modules]\n");
        for (module, module_version) in modules {
            let file = format!("{module}-{module_version}-linux-x86_64.tar.gz");
            std::fs::write(dir.join(&file), bundle_bytes(module, module_version)).unwrap();
            std::fs::write(
                dir.join(format!("{module}-{module_version}.artifact.toml")),
                format!(
                    "module = \"{module}\"\nversion = \"{module_version}\"\nplatform = \"linux-x86_64\"\nfile = \"{file}\"\n"
                ),
            )
            .unwrap();
            rules.push_str(&format!(
                "{module} = \"{}.*\"\n",
                DepotVersion::parse(module_version).unwrap().major
            ));
        }
        let rules_path = dir.join("rules.depot.toml");
        std::fs::write(&rules_path, rules).unwrap();
        let out = self.depot(&[
            "publish",
            &format!("{product}-{version}"),
            "--rules",
            rules_path.to_str().unwrap(),
            "--artifacts",
            dir.to_str().unwrap(),
            "--yes",
        ]);
        assert!(out.contains("published"), "{out}");
    }
}

/// A module bundle in the layout the module storage unpacks.
fn bundle_bytes(module: &str, version: &str) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut add = |path: String, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, data).unwrap();
    };
    add(
        format!("{module}/{module}"),
        format!("#!/bin/sh\n# {module} {version}\n").as_bytes(),
    );
    add(
        format!("{module}/.fenrir/runtime.toml"),
        b"[migrations]\ndir = \"migrations\"\n",
    );
    add(
        format!("{module}/migrations/0001_{}.sql", version.replace('.', "_")),
        b"SELECT 1;",
    );
    tar.into_inner().unwrap().finish().unwrap()
}

async fn database() -> Option<(String, PathBuf)> {
    let (Ok(admin_url), Ok(bin)) = (
        std::env::var("DEPOT_TEST_DATABASE_URL"),
        std::env::var("FENRIR_DEPOT_E2E_BIN"),
    ) else {
        eprintln!(
            "SKIPPED: DEPOT_TEST_DATABASE_URL / FENRIR_DEPOT_E2E_BIN not set (run scripts/depot-e2e.sh)"
        );
        return None;
    };
    let (client, connection) = tokio_postgres::connect(&admin_url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    let name = format!("depot_host_e2e_{}", std::process::id());
    client
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    Some((format!("{base}/{name}"), PathBuf::from(bin)))
}

async fn start_depot(database_url: &str, bin: PathBuf, work: &Path, sockets: &Path) -> DepotWorld {
    let depot = bin.join("depot");
    let server = bin.join("depot-server");
    let ca = work.join("ca");
    run(
        &depot,
        &[
            "cert",
            "--ca-dir",
            ca.to_str().unwrap(),
            "init",
            "--no-passphrase",
        ],
    );
    let issue = |name: &str, role: &str, kind: &str| {
        let out = work.join("certs").join(name);
        run(
            &depot,
            &[
                "cert",
                "--ca-dir",
                ca.to_str().unwrap(),
                "issue",
                name,
                "--role",
                role,
                "--kind",
                kind,
                "--no-p12",
                "--out",
                out.to_str().unwrap(),
            ],
        )
    };
    let joel = issue("joel", "publisher", "person");
    let host = issue("server-test", "reader", "server");
    let release = work.join("keys/release.key");
    run(&depot, &["keygen", "--out", release.to_str().unwrap()]);
    let public = std::fs::read_to_string(release.with_extension("pub")).unwrap();

    let url_file = work.join("database-url");
    private_file(&url_file, database_url);
    let socket = sockets.join("depot.sock");
    let config = work.join("depot-server.toml");
    std::fs::write(
        &config,
        format!(
            "database_url_file = \"{}\"\nblob_dir = \"{}\"\npublisher_keys = [\"{}\"]\npublish_channels = [\"beta\"]\n",
            url_file.display(),
            work.join("blobs").display(),
            public.trim()
        ),
    )
    .unwrap();
    let config_arg = config.to_str().unwrap();
    run(&server, &["--config", config_arg, "migrate"]);
    for printed in [&joel, &host] {
        let mut argv = vec!["--config".to_string(), config_arg.to_string()];
        argv.extend(server_command(printed));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        run(&server, &argv);
    }
    let server_child = Killed(
        Command::new(&server)
            .args([
                "--config",
                config_arg,
                "serve",
                "--socket",
                socket.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(|| socket.exists()).await;
    let port_file = work.join("proxy-port");
    let proxy = Killed(
        Command::new(bin.join("mtls_proxy"))
            .args([
                "--ca-dir",
                ca.to_str().unwrap(),
                "--socket",
                socket.to_str().unwrap(),
                "--port-file",
                port_file.to_str().unwrap(),
            ])
            .spawn()
            .unwrap(),
    );
    wait_for(|| port_file.exists()).await;
    let port = std::fs::read_to_string(&port_file).unwrap();
    DepotWorld {
        bin,
        work: work.to_path_buf(),
        url: format!("https://localhost:{}", port.trim()),
        _server: server_child,
        _proxy: proxy,
    }
}

async fn wait_for(check: impl Fn() -> bool) {
    for _ in 0..300 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out");
}

// ─── the host side ───────────────────────────────────────────────────────

/// `ModuleHost` over the real module storage in a temp directory; the
/// process runtime is replaced by a set of "running" modules.
struct StorageHost {
    storage: FilesystemModuleStorage,
    running: Mutex<HashSet<String>>,
    log: Mutex<Vec<String>>,
}

impl StorageHost {
    fn new(install_dir: &Path) -> Self {
        Self {
            storage: FilesystemModuleStorage::new(&ModuleStorageSection {
                install_dir: install_dir.display().to_string(),
                cache_dir: None,
            })
            .unwrap(),
            running: Mutex::new(HashSet::new()),
            log: Mutex::new(Vec::new()),
        }
    }

    fn note(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }

    async fn version(&self, module: &str) -> Option<String> {
        self.storage
            .load(&ModuleId::new(module).unwrap())
            .await
            .unwrap()
            .map(|installed| installed.manifest.version.to_string())
    }
}

#[async_trait]
impl ModuleHost for StorageHost {
    async fn modules(&self) -> Result<Vec<HostModule>, String> {
        let running = self.running.lock().unwrap().clone();
        let installed = self.storage.list().await.map_err(|e| e.to_string())?;
        Ok(installed
            .into_iter()
            .map(|module| {
                let dir = PathBuf::from(&module.path);
                let migrations = std::fs::read_to_string(dir.join(".fenrir/runtime.toml"))
                    .ok()
                    .and_then(|text| parse_migrations_dir(&text))
                    .and_then(|sub| list_sql_files_in(&dir.join(sub)).ok())
                    .unwrap_or_default();
                let is_running = running.contains(&module.manifest.id);
                HostModule {
                    name: module.manifest.id.clone(),
                    version: DepotVersion::parse(&module.manifest.version.to_string()).unwrap(),
                    sha256: module.manifest.artifact.checksum.hash.clone(),
                    local_sync: module.source.is_synchronized(),
                    running: is_running,
                    instances: usize::from(is_running),
                    migrations,
                }
            })
            .collect())
    }

    async fn backup(&self, module: &str, archive: &Path) -> Result<Option<ModuleManifest>, String> {
        let Some(installed) = self
            .storage
            .load(&ModuleId::new(module).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        // The storage keeps the installed archive next to the manifest.
        let file = installed
            .manifest
            .artifact
            .download_url
            .rsplit('/')
            .next()
            .unwrap()
            .to_string();
        std::fs::copy(
            Path::new(&installed.path).join(".fenrir-meta").join(file),
            archive,
        )
        .map_err(|e| e.to_string())?;
        Ok(Some(installed.manifest))
    }

    async fn stop(&self, module: &str) -> Result<(), String> {
        self.running.lock().unwrap().remove(module);
        self.note(format!("stop {module}"));
        Ok(())
    }

    async fn activate(&self, bundle: ModuleBundle) -> Result<(), String> {
        let id = bundle.manifest.module_id().map_err(|e| e.to_string())?;
        if let Some(installed) = self.storage.load(&id).await.map_err(|e| e.to_string())? {
            if installed.manifest.version > bundle.manifest.version {
                self.storage.remove(&id).await.map_err(|e| e.to_string())?;
            }
        }
        self.note(format!("activate {id} {}", bundle.manifest.version));
        self.storage
            .stage_and_activate(bundle, ModuleInstallSource::Distribution)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn remove(&self, module: &str) -> Result<(), String> {
        self.storage
            .remove(&ModuleId::new(module).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())
    }

    async fn apply_migrations(&self, modules: &[String]) -> Result<Vec<String>, String> {
        self.note(format!("migrate {}", modules.join(",")));
        Ok(modules.to_vec())
    }

    async fn start(&self, modules: &[String]) -> Result<(), String> {
        for module in modules {
            self.running.lock().unwrap().insert(module.clone());
        }
        self.note(format!("start {}", modules.join(",")));
        Ok(())
    }

    async fn rolling_restart(&self, module: &str) -> Result<(), String> {
        self.note(format!("rolling {module}"));
        Ok(())
    }

    async fn wait_healthy(&self, module: &str) -> Result<(), String> {
        // "Healthy" = the unpacked binary of the expected version is there.
        let installed = self
            .storage
            .load(&ModuleId::new(module).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{module} is not installed"))?;
        let binary = std::fs::read_to_string(Path::new(&installed.path).join(module))
            .map_err(|e| format!("{module}: {e}"))?;
        if binary.contains(&format!("# {module} {}", installed.manifest.version)) {
            Ok(())
        } else {
            Err(format!("{module}: unexpected files"))
        }
    }
}

fn settings(world: &DepotWorld, identity: bool) -> DepotSettings {
    let w = &world.work;
    let host = w.join("certs/server-test");
    let mut vars = vec![
        ("FENRIR_DEPOT_URL".to_string(), world.url.clone()),
        ("FENRIR_DEPOT_CHANNEL".to_string(), "stable".to_string()),
        (
            "FENRIR_DEPOT_TRUSTED_KEYS_FILE".to_string(),
            w.join("keys/release.pub").display().to_string(),
        ),
        (
            "FENRIR_DEPOT_CA_CERT_FILE".to_string(),
            w.join("ca/ca.pem").display().to_string(),
        ),
        (
            "FENRIR_DEPOT_PLATFORM".to_string(),
            "linux-x86_64".to_string(),
        ),
    ];
    if identity {
        vars.push((
            "FENRIR_DEPOT_CLIENT_CERT_FILE".to_string(),
            host.join("server-test.pem").display().to_string(),
        ));
        vars.push((
            "FENRIR_DEPOT_CLIENT_KEY_FILE".to_string(),
            host.join("server-test.key").display().to_string(),
        ));
    }
    DepotSettings::from_lookup(move |name| {
        vars.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    })
    .unwrap()
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_host_installs_upgrades_pins_hotfixes_and_rolls_back_from_a_real_depot() {
    let Some((database_url, bin)) = database().await else {
        return;
    };
    let work = tempfile::tempdir().unwrap();
    // Unix socket paths are short on macOS: the socket lives under /tmp.
    let sockets = tempfile::Builder::new()
        .prefix("dhe")
        .tempdir_in("/tmp")
        .unwrap();
    let world = start_depot(&database_url, bin, work.path(), sockets.path()).await;

    world.publish(
        "athenos",
        "1.0.0",
        &[("athenos", "1.0.0"), ("athenos-web", "1.0.0")],
    );
    world.depot(&["promote", "athenos-1.0.0", "--to", "stable", "--yes"]);
    world.publish(
        "athenos",
        "1.1.0",
        &[("athenos", "1.1.0"), ("athenos-web", "1.1.0")],
    );
    world.depot(&["promote", "athenos-1.1.0", "--to", "stable", "--yes"]);

    let settings = settings(&world, true);
    let client = DepotHttpClient::new(&settings).unwrap();
    let modules_dir = work.path().join("modules");
    let host = StorageHost::new(&modules_dir);
    let store = StateStore::new(modules_dir.join("runtime/depot"));
    let policy = settings.policy(DepotVersion::new(0, 13, 0, 0));
    let depot = Depot {
        source: &client,
        host: &host,
        store: &store,
        policy: &policy,
    };

    // Without a client certificate there is no TLS session at all.
    let anonymous = DepotHttpClient::new(&self::settings(&world, false)).unwrap();
    let err = anonymous
        .distribution(&"athenos-1.0.0".parse().unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, DepotError::Unreachable(_)), "{err}");

    // 1. install distribution athenos-1.0.0
    let prepared = prepare_distribution(
        &depot,
        &DistributionRequest::Exact("athenos-1.0.0".parse().unwrap()),
        &quiet,
    )
    .await
    .unwrap();
    assert_eq!(prepared.plan.changes().count(), 2);
    assert!(prepared.downloaded > 0);
    let outcome = execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(
        outcome.changed,
        vec!["athenos - → 1.0.0", "athenos-web - → 1.0.0"]
    );
    assert_eq!(host.version("athenos").await.as_deref(), Some("1.0.0"));
    assert!(
        modules_dir.join("athenos/athenos").exists(),
        "unpacked into the modules dir"
    );
    let state = store.load().unwrap();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.0.0");
    assert_eq!(state.current.as_ref().unwrap().channel, "stable");

    // 2. status distribution: something newer in the followed channel.
    let newer = newer_distribution(&client, &policy, &"athenos-1.0.0".parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(newer.id().to_string(), "athenos-1.1.0");

    // 3. install module athenos-web 1.1.* (hotfix, vouched for by 1.1.0).
    let prepared = prepare_module(
        &depot,
        &"athenos-web".parse().unwrap(),
        &"1.1.*".parse().unwrap(),
        &quiet,
    )
    .await
    .unwrap();
    assert!(
        matches!(&prepared.intent, Intent::Module { candidate } if candidate.from.to_string() == "athenos-1.1.0")
    );
    execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(host.version("athenos-web").await.as_deref(), Some("1.1.0"));
    let state = store.load().unwrap();
    assert_eq!(state.overrides["athenos-web"].from, "athenos-1.1.0");

    // 4. pin module athenos-web 1.1.0, then install distribution athenos 1.*
    update_state(&store, |state| {
        state
            .pins
            .insert("athenos-web".to_string(), DepotVersion::new(1, 1, 0, 0));
        Ok(())
    })
    .unwrap();
    let request = DistributionRequest::parse(&["athenos", "1.*"]).unwrap();
    let prepared = prepare_distribution(&depot, &request, &quiet)
        .await
        .unwrap();
    assert_eq!(prepared.plan.title, "athenos-1.1.0");
    let web = prepared
        .plan
        .entries
        .iter()
        .find(|e| e.module == "athenos-web")
        .unwrap();
    assert_eq!(web.action, Action::Pinned);
    execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(host.version("athenos").await.as_deref(), Some("1.1.0"));
    let state = store.load().unwrap();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.0");
    assert_eq!(state.previous.as_ref().unwrap().id, "athenos-1.0.0");
    assert!(state.backup.is_some());

    // 5. rollback: from the local backup, back to athenos 1.0.0.
    let prepared = prepare_rollback(&depot, &quiet).await.unwrap();
    assert!(matches!(prepared.intent, Intent::Restore { .. }));
    execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(host.version("athenos").await.as_deref(), Some("1.0.0"));
    assert_eq!(
        host.version("athenos-web").await.as_deref(),
        Some("1.1.0"),
        "pinned"
    );
    let state = store.load().unwrap();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.0.0");

    // 6. A signed yank: athenos-1.1.0 is refused, the range falls back.
    world.depot(&["yank", "athenos-1.1.0", "bad build", "--yes"]);
    let err = prepare_distribution(
        &depot,
        &DistributionRequest::Exact("athenos-1.1.0".parse().unwrap()),
        &quiet,
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("athenos-1.1.0 is yanked: bad build"),
        "{err}"
    );
    let prepared = prepare_distribution(&depot, &request, &quiet)
        .await
        .unwrap();
    assert_eq!(prepared.plan.title, "athenos-1.0.0");
    assert!(!prepared.plan.has_changes());
    assert!(
        prepared.skipped[0].contains("yanked"),
        "{:?}",
        prepared.skipped
    );

    // 7. A beta-only release is not for this stable host.
    world.publish(
        "athenos",
        "1.2.0",
        &[("athenos", "1.2.0"), ("athenos-web", "1.2.0")],
    );
    let err = prepare_distribution(
        &depot,
        &DistributionRequest::Exact("athenos-1.2.0".parse().unwrap()),
        &quiet,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("channel beta"), "{err}");
    let log = host.log.lock().unwrap().clone();
    assert!(
        log.iter().any(|l| l.starts_with("migrate athenos")),
        "{log:?}"
    );
}
