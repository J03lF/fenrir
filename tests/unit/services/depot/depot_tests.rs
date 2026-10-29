//! The depot pipeline against an in-memory depot and a fake module host:
//! planning, pins and overrides, channel rules, yanks, requirements,
//! tamper detection, install / rollback / hotfix and the automatic rollback.
//! The real runtime (`ModuleServiceHost`) and the real mTLS client are
//! covered by `tests/depot_e2e.rs`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use depot_protocol::api::{
    ArtifactInfo, ArtifactUsage, DistributionDetail, DistributionSummary, IncludeSummary,
    ItemSummary, ModuleInfo, PromotionInfo, SearchResponse, UsageRef, YankInfo,
};
use depot_protocol::manifest::{ModuleRequirement, MANIFEST_FORMAT};
use depot_protocol::rule::VersionPattern;
use depot_protocol::statement::DistributionRef;
use depot_protocol::{
    Channel, DistributionId, Manifest, ManifestInclude, ManifestModule, Name, Requires, Rule,
    SecretKey, Sha256Digest, Statement, StatementAction, Timestamp, TrustedKeys,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use semver::Version;

use super::blob::{BlobCache, BlobWriter};
use super::bundle;
use super::installer::{
    execute, prepare_distribution, prepare_module, prepare_rollback, update_state, Depot, Intent,
};
use super::plan::{plan_distribution, plan_module, Action, Restart};
use super::ports::{DepotSource, HostModule, ModuleHost};
use super::state::{HostState, ModuleOverride, StateStore};
use super::verify::{newer_distribution, DistributionRequest, HostPolicy, TargetModule};
use super::DepotError;
use crate::domain::module::{
    ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleBundle, ModuleChecksum, ModuleManifest,
    ModuleSignatureDescriptor, SignatureAlgorithm,
};

// ─── fixtures ────────────────────────────────────────────────────────────

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn name(text: &str) -> Name {
    text.parse().unwrap()
}

fn at() -> Timestamp {
    Timestamp::from_unix_seconds(1_800_000_000).unwrap()
}

fn module_manifest(module: &str, version: &str) -> ModuleManifest {
    ModuleManifest {
        id: module.to_string(),
        version: v(version),
        title: Some(format!("{module} title")),
        description: None,
        fenrir_version: None,
        authors: Vec::new(),
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
        dependencies: Vec::new(),
        tags: Vec::new(),
        published_at: None,
    }
}

/// A module bundle as the storage expects it: gzip tar, one top directory,
/// its own manifest, a runtime manifest with migrations.
fn bundle_bytes(module: &str, version: &str, migrations: &[&str]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut add = |path: String, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, data).unwrap();
    };
    add(
        format!("{module}/{module}"),
        format!("binary {module} {version}").as_bytes(),
    );
    add(
        format!("{module}/.fenrir-meta/manifest.json"),
        &serde_json::to_vec(&module_manifest(module, version)).unwrap(),
    );
    add(
        format!("{module}/.fenrir/runtime.toml"),
        b"[migrations]\ndir = \"migrations\"\n",
    );
    for file in migrations {
        add(format!("{module}/migrations/{file}"), b"SELECT 1;");
    }
    tar.into_inner().unwrap().finish().unwrap()
}

struct Built {
    module: ManifestModule,
    bytes: Vec<u8>,
}

fn built(module: &str, version: &str, migrations: &[&str], requires: Requires) -> Built {
    let bytes = bundle_bytes(module, version, migrations);
    Built {
        module: ManifestModule {
            module: name(module),
            rule: format!("{}.*", v(version).major).parse::<Rule>().unwrap(),
            version: v(version),
            platform: "linux-x86_64".parse().unwrap(),
            sha256: Sha256Digest::of(&bytes),
            size: bytes.len() as u64,
            optional: false,
            requires,
        },
        bytes,
    }
}

fn plain(module: &str, version: &str) -> Built {
    built(module, version, &[], Requires::default())
}

fn manifest(
    product: &str,
    version: &str,
    modules: &[&Built],
    includes: Vec<ManifestInclude>,
) -> Manifest {
    let mut modules: Vec<ManifestModule> = modules.iter().map(|b| b.module.clone()).collect();
    modules.sort_by(|a, b| a.module.cmp(&b.module));
    Manifest {
        format: MANIFEST_FORMAT.to_string(),
        product: name(product),
        version: v(version),
        channel: Channel::Beta,
        platform: "linux-x86_64".parse().unwrap(),
        created_at: at(),
        modules,
        includes,
    }
}

/// A signed detail, promoted to `channel`, optionally yanked.
fn detail(
    secret: &SecretKey,
    manifest: &Manifest,
    channel: Channel,
    yank: Option<&str>,
) -> DistributionDetail {
    let envelope = secret.sign(manifest).unwrap();
    let digest = envelope.payload_digest().unwrap();
    let reference = DistributionRef {
        product: manifest.product.clone(),
        version: manifest.version.clone(),
        manifest_sha256: digest,
    };
    let mut promotions = Vec::new();
    let mut from = manifest.channel;
    for to in Channel::ALL
        .into_iter()
        .filter(|c| *c > manifest.channel && *c <= channel)
    {
        let statement = Statement::new(
            at(),
            StatementAction::PromoteDistribution {
                distribution: reference.clone(),
                channel: to,
            },
        );
        promotions.push(PromotionInfo {
            from,
            to,
            at: at(),
            statement: secret.sign(&statement).unwrap(),
        });
        from = to;
    }
    let yank_statement = yank.map(|reason| {
        secret
            .sign(&Statement::new(
                at(),
                StatementAction::YankDistribution {
                    distribution: reference.clone(),
                    reason: reason.to_string(),
                },
            ))
            .unwrap()
    });
    DistributionDetail {
        summary: DistributionSummary {
            product: manifest.product.clone(),
            version: manifest.version.clone(),
            channel,
            platform: manifest.platform.clone(),
            manifest_sha256: digest,
            published_at: at(),
            yanked: yank.map(|reason| YankInfo {
                reason: reason.to_string(),
                at: at(),
            }),
            modules: manifest
                .modules
                .iter()
                .map(|m| ItemSummary {
                    module: m.module.clone(),
                    rule: m.rule.clone(),
                    version: m.version.clone(),
                    platform: m.platform.clone(),
                    sha256: m.sha256,
                    size: m.size,
                    optional: m.optional,
                })
                .collect(),
            includes: manifest
                .includes
                .iter()
                .map(|i| IncludeSummary {
                    product: i.product.clone(),
                    rule: i.rule.clone(),
                    version: i.version.clone(),
                    manifest_sha256: i.manifest_sha256,
                })
                .collect(),
        },
        manifest: envelope,
        promotions,
        yank_statement,
    }
}

fn include_of(detail: &DistributionDetail) -> ManifestInclude {
    ManifestInclude {
        product: detail.summary.product.clone(),
        rule: format!("{}.*", detail.summary.version.major)
            .parse()
            .unwrap(),
        version: detail.summary.version.clone(),
        manifest_sha256: detail.summary.manifest_sha256,
    }
}

/// The depot in memory.
#[derive(Default)]
struct FakeDepot {
    details: Mutex<BTreeMap<String, DistributionDetail>>,
    blobs: Mutex<HashMap<String, Vec<u8>>>,
    blob_requests: Mutex<usize>,
}

impl FakeDepot {
    fn publish(&self, detail: DistributionDetail, bundles: &[&Built]) {
        for bundle in bundles {
            self.blobs
                .lock()
                .unwrap()
                .insert(bundle.module.sha256.to_hex(), bundle.bytes.clone());
        }
        self.details
            .lock()
            .unwrap()
            .insert(detail.summary.id().to_string(), detail);
    }

    fn requests(&self) -> usize {
        *self.blob_requests.lock().unwrap()
    }
}

#[async_trait]
impl DepotSource for FakeDepot {
    async fn distribution(
        &self,
        id: &DistributionId,
    ) -> Result<Option<DistributionDetail>, DepotError> {
        Ok(self.details.lock().unwrap().get(&id.to_string()).cloned())
    }

    async fn distributions(&self, product: &Name) -> Result<Vec<DistributionSummary>, DepotError> {
        Ok(self
            .details
            .lock()
            .unwrap()
            .values()
            .filter(|d| &d.summary.product == product)
            .map(|d| d.summary.clone())
            .collect())
    }

    async fn module(&self, module: &Name) -> Result<Option<ModuleInfo>, DepotError> {
        let details = self.details.lock().unwrap();
        let mut versions: BTreeMap<String, ArtifactUsage> = BTreeMap::new();
        for detail in details.values() {
            for item in detail
                .summary
                .modules
                .iter()
                .filter(|m| &m.module == module)
            {
                let usage =
                    versions
                        .entry(item.version.to_string())
                        .or_insert_with(|| ArtifactUsage {
                            artifact: ArtifactInfo {
                                module: item.module.clone(),
                                version: item.version.clone(),
                                platform: item.platform.clone(),
                                sha256: item.sha256,
                                size: item.size,
                                requires: Requires::default(),
                                notes: None,
                                uploaded_at: at(),
                                yanked: None,
                            },
                            used_in: Vec::new(),
                        });
                usage.used_in.push(UsageRef {
                    distribution: detail.summary.id(),
                    rule: item.rule.clone(),
                    channel: detail.summary.channel,
                    yanked: detail.summary.yanked.is_some(),
                });
            }
        }
        if versions.is_empty() {
            return Ok(None);
        }
        Ok(Some(ModuleInfo {
            name: module.clone(),
            description: String::new(),
            repository_url: None,
            versions: versions.into_values().collect(),
        }))
    }

    async fn search(&self, query: &str) -> Result<SearchResponse, DepotError> {
        Ok(SearchResponse {
            query: query.to_string(),
            distributions: Vec::new(),
            modules: Vec::new(),
        })
    }

    async fn blob(
        &self,
        digest: &Sha256Digest,
        _size: u64,
        sink: &mut BlobWriter,
    ) -> Result<(), DepotError> {
        *self.blob_requests.lock().unwrap() += 1;
        let bytes = self
            .blobs
            .lock()
            .unwrap()
            .get(&digest.to_hex())
            .cloned()
            .ok_or_else(|| DepotError::NotFound(format!("blob {digest}")))?;
        let (first, second) = bytes.split_at(bytes.len() / 2);
        sink.write(first)?;
        sink.write(second)
    }
}

#[derive(Debug, Clone)]
struct FakeModule {
    version: Version,
    sha256: String,
    running: bool,
    instances: usize,
    migrations: Vec<String>,
    archive: Vec<u8>,
    manifest: ModuleManifest,
}

/// The module runtime in memory; `unhealthy` holds `name@version`.
#[derive(Default)]
struct FakeHost {
    modules: Mutex<BTreeMap<String, FakeModule>>,
    log: Mutex<Vec<String>>,
    unhealthy: Mutex<HashSet<String>>,
}

impl FakeHost {
    fn with(modules: &[(&str, &str, usize, &[&str])]) -> Self {
        let host = Self::default();
        for (module, version, instances, migrations) in modules {
            host.modules.lock().unwrap().insert(
                (*module).to_string(),
                FakeModule {
                    version: v(version),
                    sha256: "copied-by-rsync".to_string(),
                    running: true,
                    instances: *instances,
                    migrations: migrations.iter().map(|m| (*m).to_string()).collect(),
                    archive: bundle_bytes(module, version, migrations),
                    manifest: module_manifest(module, version),
                },
            );
        }
        host
    }

    fn version(&self, module: &str) -> Option<String> {
        self.modules
            .lock()
            .unwrap()
            .get(module)
            .map(|m| m.version.to_string())
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn note(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }
}

#[async_trait]
impl ModuleHost for FakeHost {
    async fn modules(&self) -> Result<Vec<HostModule>, String> {
        Ok(self
            .modules
            .lock()
            .unwrap()
            .iter()
            .map(|(name, m)| HostModule {
                name: name.clone(),
                version: m.version.clone(),
                sha256: m.sha256.clone(),
                local_sync: false,
                running: m.running,
                instances: m.instances,
                migrations: m.migrations.clone(),
            })
            .collect())
    }

    async fn backup(&self, module: &str, archive: &Path) -> Result<Option<ModuleManifest>, String> {
        let modules = self.modules.lock().unwrap();
        let Some(m) = modules.get(module) else {
            return Ok(None);
        };
        std::fs::write(archive, &m.archive).map_err(|e| e.to_string())?;
        let mut manifest = m.manifest.clone();
        manifest.artifact.checksum.hash = m.sha256.clone();
        Ok(Some(manifest))
    }

    async fn stop(&self, module: &str) -> Result<(), String> {
        if let Some(m) = self.modules.lock().unwrap().get_mut(module) {
            m.running = false;
        }
        self.note(format!("stop {module}"));
        Ok(())
    }

    async fn activate(&self, bundle: ModuleBundle) -> Result<(), String> {
        let info = bundle::inspect(&bundle.archive).map_err(|e| e.to_string())?;
        let module = bundle.manifest.id.clone();
        self.note(format!("activate {module} {}", bundle.manifest.version));
        let mut modules = self.modules.lock().unwrap();
        let instances = modules.get(&module).map_or(1, |m| m.instances);
        let running = modules.get(&module).is_some_and(|m| m.running);
        modules.insert(
            module,
            FakeModule {
                version: bundle.manifest.version.clone(),
                sha256: bundle.manifest.artifact.checksum.hash.clone(),
                running,
                instances,
                migrations: info.migrations,
                archive: bundle.archive,
                manifest: bundle.manifest,
            },
        );
        Ok(())
    }

    async fn remove(&self, module: &str) -> Result<(), String> {
        self.modules.lock().unwrap().remove(module);
        self.note(format!("remove {module}"));
        Ok(())
    }

    async fn apply_migrations(&self, modules: &[String]) -> Result<Vec<String>, String> {
        self.note(format!("migrate {}", modules.join(",")));
        Ok(modules.iter().map(|m| format!("{m}:migrations")).collect())
    }

    async fn start(&self, modules: &[String]) -> Result<(), String> {
        let mut all = self.modules.lock().unwrap();
        for module in modules {
            if let Some(m) = all.get_mut(module) {
                m.running = true;
            }
        }
        drop(all);
        self.note(format!("start {}", modules.join(",")));
        Ok(())
    }

    async fn rolling_restart(&self, module: &str) -> Result<(), String> {
        self.note(format!("rolling {module}"));
        Ok(())
    }

    async fn wait_healthy(&self, module: &str) -> Result<(), String> {
        let version = self.version(module).unwrap_or_default();
        if self
            .unhealthy
            .lock()
            .unwrap()
            .contains(&format!("{module}@{version}"))
        {
            return Err(format!("{module} {version} health check failed"));
        }
        Ok(())
    }
}

struct World {
    secret: SecretKey,
    depot: FakeDepot,
    host: FakeHost,
    store: StateStore,
    policy: HostPolicy,
    _dir: tempfile::TempDir,
}

impl World {
    fn new(host: FakeHost) -> Self {
        let secret = SecretKey::from_seed(&[7; 32]);
        let dir = tempfile::tempdir().unwrap();
        Self {
            policy: HostPolicy {
                trusted: TrustedKeys::new(vec![secret.public_key()]),
                channel: Channel::Stable,
                platform: "linux-x86_64".parse().unwrap(),
                fenrir: v("0.13.0"),
            },
            secret,
            depot: FakeDepot::default(),
            host,
            store: StateStore::new(dir.path().join("depot")),
            _dir: dir,
        }
    }

    fn depot(&self) -> Depot<'_> {
        Depot {
            source: &self.depot,
            host: &self.host,
            store: &self.store,
            policy: &self.policy,
        }
    }

    /// Publishes `product-version` with these modules at `channel`.
    fn release(
        &self,
        product: &str,
        version: &str,
        channel: Channel,
        modules: &[&Built],
    ) -> DistributionDetail {
        let detail = detail(
            &self.secret,
            &manifest(product, version, modules, Vec::new()),
            channel,
            None,
        );
        self.depot.publish(detail.clone(), modules);
        detail
    }

    fn state(&self) -> HostState {
        self.store.load().unwrap()
    }
}

fn quiet(_: String) {}

fn exact(id: &str) -> DistributionRequest {
    DistributionRequest::Exact(id.parse().unwrap())
}

fn range(product: &str, rule: &str) -> DistributionRequest {
    DistributionRequest::Range {
        product: name(product),
        rule: rule.parse().unwrap(),
    }
}

fn target(module: &str, version: &str, sha: &str) -> TargetModule {
    TargetModule {
        name: module.to_string(),
        rule: "*".to_string(),
        version: v(version),
        platform: "linux-x86_64".to_string(),
        sha256: Sha256Digest::of(sha.as_bytes()),
        size: 10,
        optional: false,
        requires: Requires::default(),
        via: None,
    }
}

fn installed(
    module: &str,
    version: &str,
    sha: &str,
    running: bool,
    instances: usize,
) -> HostModule {
    HostModule {
        name: module.to_string(),
        version: v(version),
        sha256: Sha256Digest::of(sha.as_bytes()).to_hex(),
        local_sync: false,
        running,
        instances,
        migrations: Vec::new(),
    }
}

// ─── planning ────────────────────────────────────────────────────────────

#[test]
fn the_plan_says_per_module_what_happens_and_how_it_restarts() {
    let mut optional = target("search-service", "1.0.0", "s");
    optional.optional = true;
    let targets = vec![
        target("athenos", "0.13.0", "a13"),
        target("athenos-api", "0.13.0", "api13"),
        target("athenos-web", "0.12.5", "web-other-build"),
        target("auth-service", "3.1.0", "auth31"),
        target("notification-service", "6.2.0", "n62"),
        target("old-service", "1.0.0", "old10"),
        optional,
    ];
    let host = vec![
        installed("athenos", "0.12.5", "a125", true, 1),
        installed("athenos-api", "0.12.4", "api124", true, 2),
        installed("athenos-web", "0.12.5", "web125", false, 0),
        installed("auth-service", "3.1.0", "auth31", true, 1),
        installed("old-service", "2.0.0", "old20", true, 1),
        installed("legacy", "0.1.0", "legacy", true, 1),
    ];
    let plan = plan_distribution("athenos-0.13.0", &targets, &host, &HostState::default());
    let row = |m: &str| plan.entries.iter().find(|e| e.module == m).unwrap().clone();
    assert_eq!(row("athenos").action, Action::Update);
    assert_eq!(row("athenos").restart, Restart::Restart);
    assert_eq!(row("athenos-api").restart, Restart::Rolling);
    assert_eq!(row("athenos-web").action, Action::Replace);
    assert_eq!(row("athenos-web").restart, Restart::Start);
    assert_eq!(row("auth-service").action, Action::Unchanged);
    assert_eq!(row("notification-service").action, Action::Install);
    assert_eq!(row("old-service").action, Action::Downgrade);
    assert_eq!(row("search-service").action, Action::OptionalSkipped);
    assert_eq!(row("legacy").action, Action::Kept);
    assert!(plan.problems.is_empty(), "{:?}", plan.problems);
    assert_eq!(plan.changes().count(), 5);
    assert_eq!(plan.download_size(), 50);
}

#[test]
fn pins_are_kept_and_checked_against_what_needs_them() {
    let mut athenos = target("athenos", "0.13.0", "a13");
    athenos.requires = Requires {
        fenrir: None,
        modules: vec![ModuleRequirement {
            module: name("athenos-web"),
            rule: ">=0.13".parse().unwrap(),
        }],
    };
    let targets = vec![athenos, target("athenos-web", "0.13.0", "web13")];
    let host = vec![
        installed("athenos", "0.12.5", "a125", true, 1),
        installed("athenos-web", "0.12.2", "web122", true, 1),
    ];
    let mut state = HostState::default();
    state.pins.insert("athenos-web".to_string(), v("0.12.2"));
    let plan = plan_distribution("athenos-0.13.0", &targets, &host, &state);
    let web = plan
        .entries
        .iter()
        .find(|e| e.module == "athenos-web")
        .unwrap();
    assert_eq!(web.action, Action::Pinned);
    assert_eq!(web.target, Some(v("0.12.2")));
    assert_eq!(web.note.as_deref(), Some("distribution has 0.13.0"));
    assert_eq!(
        plan.problems,
        vec!["athenos 0.13.0 requires athenos-web >=0.13, but athenos-web stays pinned at 0.12.2"]
    );

    // Without the requirement the pin is simply kept.
    let targets = vec![
        target("athenos", "0.13.0", "a13"),
        target("athenos-web", "0.13.0", "web13"),
    ];
    let plan = plan_distribution("athenos-0.13.0", &targets, &host, &state);
    assert!(plan.problems.is_empty());
    assert_eq!(
        plan.changes()
            .map(|e| e.module.as_str())
            .collect::<Vec<_>>(),
        vec!["athenos"]
    );
}

#[test]
fn overrides_end_when_the_distribution_takes_over_and_hotfixes_respect_pins() {
    let mut state = HostState::default();
    state.overrides.insert(
        "notification-service".to_string(),
        ModuleOverride {
            version: v("7.0.2"),
            sha256: "x".to_string(),
            size: 1,
            from: "athenos-0.13.1".to_string(),
            installed_at: "2026-10-29T12:00:00Z".to_string(),
        },
    );
    let targets = vec![target("notification-service", "7.0.3", "n703")];
    let host = vec![installed("notification-service", "7.0.2", "n702", true, 1)];
    let plan = plan_distribution("athenos-0.13.2", &targets, &host, &state);
    assert_eq!(
        plan.entries[0].note.as_deref(),
        Some("ends override 7.0.2 (athenos-0.13.1)")
    );

    state
        .pins
        .insert("notification-service".to_string(), v("7.0.2"));
    let plan = plan_module(&targets[0], &host, &state);
    assert_eq!(
        plan.problems,
        vec!["notification-service is pinned at 7.0.2; `unpin module notification-service` first"]
    );
}

// ─── selection: channel, yank, requirements, platform ───────────────────

#[tokio::test]
async fn channel_rules_pick_the_newest_match_in_the_hosts_channel_or_above() {
    let world = World::new(FakeHost::default());
    world.release(
        "athenos",
        "1.0.0",
        Channel::Prod,
        &[&plain("athenos", "1.0.0")],
    );
    world.release(
        "athenos",
        "1.1.0",
        Channel::Stable,
        &[&plain("athenos", "1.1.0")],
    );
    world.release(
        "athenos",
        "1.2.0",
        Channel::Beta,
        &[&plain("athenos", "1.2.0")],
    );
    let depot = world.depot();

    let prepared = prepare_distribution(&depot, &range("athenos", "1.*"), &quiet)
        .await
        .unwrap();
    assert_eq!(prepared.plan.title, "athenos-1.1.0");
    assert_eq!(prepared.skipped.len(), 1);
    assert!(
        prepared.skipped[0]
            .contains("athenos-1.2.0 is in channel beta (signed); this host follows stable"),
        "{:?}",
        prepared.skipped
    );

    let err = prepare_distribution(&depot, &exact("athenos-1.2.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DepotError::Refused(m) if m.contains("channel beta")),
        "{err}"
    );

    // A beta host takes the beta release.
    let mut beta = World::new(FakeHost::default());
    beta.policy.channel = Channel::Beta;
    beta.release(
        "athenos",
        "1.2.0",
        Channel::Beta,
        &[&plain("athenos", "1.2.0")],
    );
    let prepared = prepare_distribution(&beta.depot(), &range("athenos", "1.*"), &quiet)
        .await
        .unwrap();
    assert_eq!(prepared.plan.title, "athenos-1.2.0");

    let err = prepare_distribution(&depot, &range("athenos", "2.*"), &quiet)
        .await
        .unwrap_err();
    assert!(matches!(err, DepotError::NotFound(_)), "{err}");
}

#[tokio::test]
async fn yanked_distributions_are_refused_and_skipped() {
    let world = World::new(FakeHost::default());
    world.release(
        "athenos",
        "1.0.0",
        Channel::Stable,
        &[&plain("athenos", "1.0.0")],
    );
    let yanked_module = plain("athenos", "1.1.0");
    let yanked = detail(
        &world.secret,
        &manifest("athenos", "1.1.0", &[&yanked_module], Vec::new()),
        Channel::Stable,
        Some("broken migration"),
    );
    world.depot.publish(yanked, &[&yanked_module]);
    let depot = world.depot();

    let err = prepare_distribution(&depot, &exact("athenos-1.1.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DepotError::Refused(m) if m.contains("athenos-1.1.0 is yanked: broken migration")),
        "{err}"
    );
    let prepared = prepare_distribution(&depot, &range("athenos", "1.*"), &quiet)
        .await
        .unwrap();
    assert_eq!(prepared.plan.title, "athenos-1.0.0");
    assert!(
        prepared.skipped[0].contains("yanked"),
        "{:?}",
        prepared.skipped
    );
    assert_eq!(
        world.depot.requests(),
        1,
        "only the chosen bundle is downloaded"
    );
}

#[tokio::test]
async fn a_requirement_on_a_newer_host_fails_with_a_clear_message() {
    let world = World::new(FakeHost::default());
    let needs_new_host = built(
        "athenos",
        "1.0.0",
        &[],
        Requires {
            fenrir: Some(">=0.14, <0.16".parse().unwrap()),
            modules: Vec::new(),
        },
    );
    world.release("athenos", "1.0.0", Channel::Stable, &[&needs_new_host]);
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains(
            "athenos 1.0.0 requires fenrir >=0.14, <0.16, but this host runs fenrir 0.13.0"
        ),
        "{text}"
    );
    assert_eq!(world.depot.requests(), 0, "nothing downloaded");
}

#[tokio::test]
async fn includes_are_verified_installed_and_the_host_product_is_not() {
    let world = World::new(FakeHost::default());
    let fenrir_bundle = plain("fenrir", "0.13.0");
    let fenrir = detail(
        &world.secret,
        &manifest("fenrir", "0.13.0", &[&fenrir_bundle], Vec::new()),
        Channel::Stable,
        None,
    );
    world.depot.publish(fenrir.clone(), &[&fenrir_bundle]);
    let auth = plain("auth-service", "3.1.0");
    let platform = detail(
        &world.secret,
        &manifest("platform", "0.5.0", &[&auth], vec![include_of(&fenrir)]),
        Channel::Stable,
        None,
    );
    world.depot.publish(platform.clone(), &[&auth]);
    let athenos = plain("athenos", "1.0.0");
    world.depot.publish(
        detail(
            &world.secret,
            &manifest("athenos", "1.0.0", &[&athenos], vec![include_of(&platform)]),
            Channel::Stable,
            None,
        ),
        &[&athenos],
    );
    let prepared = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap();
    let modules: Vec<_> = prepared
        .plan
        .entries
        .iter()
        .map(|e| (e.module.as_str(), e.via.as_deref()))
        .collect();
    assert_eq!(
        modules,
        vec![("athenos", None), ("auth-service", Some("platform-0.5.0"))]
    );
    assert_eq!(prepared.host_includes, vec!["fenrir-0.13.0"]);

    // An include that is not the manifest the parent signed.
    let other_platform = detail(
        &world.secret,
        &manifest(
            "platform",
            "0.5.0",
            &[&plain("auth-service", "3.2.0")],
            Vec::new(),
        ),
        Channel::Stable,
        None,
    );
    world.depot.publish(other_platform, &[]);
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DepotError::Verification(m) if m.contains("is not the manifest it references")),
        "{err}"
    );
}

#[tokio::test]
async fn a_module_for_another_platform_is_refused() {
    let world = World::new(FakeHost::default());
    let mut arm = plain("athenos", "1.0.0");
    arm.module.platform = "linux-aarch64".parse().unwrap();
    let mut for_arm = manifest("athenos", "1.0.0", &[&arm], Vec::new());
    for_arm.platform = "linux-aarch64".parse().unwrap();
    world.depot.publish(
        detail(&world.secret, &for_arm, Channel::Stable, None),
        &[&arm],
    );
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("is built for linux-aarch64, this host is linux-x86_64"),
        "{err}"
    );
}

// ─── tamper detection ────────────────────────────────────────────────────

#[tokio::test]
async fn a_bundle_with_the_wrong_sha256_is_rejected_before_anything_changes() {
    let world = World::new(FakeHost::with(&[("athenos", "0.9.0", 1, &[])]));
    let good = plain("athenos", "1.0.0");
    world.release("athenos", "1.0.0", Channel::Stable, &[&good]);
    // The depot serves other bytes under the signed digest.
    world.depot.blobs.lock().unwrap().insert(
        good.module.sha256.to_hex(),
        bundle_bytes("athenos", "6.6.6", &[]),
    );
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    assert!(matches!(&err, DepotError::Bundle(_)), "{err}");
    assert!(err.to_string().contains("athenos 1.0.0"), "{err}");
    assert!(
        world.host.log().is_empty(),
        "nothing touched: {:?}",
        world.host.log()
    );
    assert_eq!(world.host.version("athenos").as_deref(), Some("0.9.0"));
    let cache = BlobCache::new(world.store.blob_dir());
    assert!(!cache.path(&good.module.sha256).exists());
    assert!(!world.store.state_path().exists());
}

#[tokio::test]
async fn a_bad_signature_an_unknown_key_and_a_lying_summary_are_rejected() {
    let world = World::new(FakeHost::default());
    let bundle = plain("athenos", "1.0.0");
    let mut tampered = detail(
        &world.secret,
        &manifest("athenos", "1.0.0", &[&bundle], Vec::new()),
        Channel::Stable,
        None,
    );
    tampered.manifest.payload["modules"][0]["size"] = serde_json::json!(1);
    world.depot.publish(tampered, &[&bundle]);
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DepotError::Verification(m) if m.contains("signature does not match")),
        "{err}"
    );

    let stranger = SecretKey::from_seed(&[9; 32]);
    world.depot.publish(
        detail(
            &stranger,
            &manifest("athenos", "1.0.1", &[&bundle], Vec::new()),
            Channel::Stable,
            None,
        ),
        &[&bundle],
    );
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.1"), &quiet)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DepotError::Verification(m) if m.contains("not a trusted publisher key")),
        "{err}"
    );

    // The summary claims prod; the signed statements only reach stable.
    let mut lying = detail(
        &world.secret,
        &manifest("athenos", "1.0.2", &[&bundle], Vec::new()),
        Channel::Stable,
        None,
    );
    lying.summary.channel = Channel::Prod;
    world.depot.publish(lying, &[&bundle]);
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.2"), &quiet)
        .await
        .unwrap_err();
    assert!(matches!(err, DepotError::Verification(_)), "{err}");
    assert_eq!(world.depot.requests(), 0);
}

#[tokio::test]
async fn a_bundle_whose_own_manifest_names_another_module_is_rejected() {
    let world = World::new(FakeHost::default());
    let mut swapped = plain("athenos", "1.0.0");
    swapped.bytes = bundle_bytes("auth-service", "1.0.0", &[]);
    swapped.module.sha256 = Sha256Digest::of(&swapped.bytes);
    swapped.module.size = swapped.bytes.len() as u64;
    world.release("athenos", "1.0.0", Channel::Stable, &[&swapped]);
    let err = prepare_distribution(&world.depot(), &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("the bundle's own manifest says auth-service 1.0.0"),
        "{err}"
    );
}

#[test]
fn bundles_with_links_or_parent_paths_are_rejected() {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_cksum();
    tar.append_link(&mut header, "m/escape", "/etc/passwd")
        .unwrap();
    let bytes = tar.into_inner().unwrap().finish().unwrap();
    let err = bundle::inspect(&bytes).unwrap_err();
    assert!(err.to_string().contains("link or special file"), "{err}");

    let mut raw = tar::Header::new_old();
    raw.set_size(1);
    raw.set_entry_type(tar::EntryType::Regular);
    {
        let name = b"m/../../evil";
        raw.as_old_mut().name[..name.len()].copy_from_slice(name);
    }
    raw.set_cksum();
    let mut plain_tar = Vec::new();
    plain_tar.extend_from_slice(raw.as_bytes());
    plain_tar.extend_from_slice(b"x");
    plain_tar.resize(plain_tar.len() + 511, 0);
    plain_tar.resize(plain_tar.len() + 1024, 0);
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&plain_tar).unwrap();
    let err = bundle::inspect(&gz.finish().unwrap()).unwrap_err();
    assert!(err.to_string().contains("parent path"), "{err}");

    let err = bundle::inspect(b"not gzip at all").unwrap_err();
    assert!(err.to_string().contains("gzip tar"), "{err}");

    let info = bundle::inspect(&bundle_bytes(
        "athenos",
        "1.0.0",
        &["0002_b.sql", "0001_a.sql"],
    ))
    .unwrap();
    assert_eq!(info.migrations, vec!["0001_a.sql", "0002_b.sql"]);
    assert_eq!(info.manifest.unwrap().id, "athenos");
}

// ─── the pipeline ────────────────────────────────────────────────────────

#[tokio::test]
async fn install_upgrade_pin_hotfix_and_both_kinds_of_rollback() {
    let world = World::new(FakeHost::with(&[
        ("athenos", "0.9.0", 1, &["0001_init.sql"]),
        ("athenos-web", "0.9.0", 2, &[]),
    ]));
    let a100 = built("athenos", "1.0.0", &["0001_init.sql"], Requires::default());
    let w100 = plain("athenos-web", "1.0.0");
    world.release("athenos", "1.0.0", Channel::Stable, &[&a100, &w100]);
    let a110 = built(
        "athenos",
        "1.1.0",
        &["0001_init.sql", "0002_links.sql"],
        Requires::default(),
    );
    let w110 = plain("athenos-web", "1.1.0");
    world.release("athenos", "1.1.0", Channel::Stable, &[&a110, &w110]);
    let depot = world.depot();

    // 1. First install over modules copied by rsync: everything replaced.
    let prepared = prepare_distribution(&depot, &exact("athenos-1.0.0"), &quiet)
        .await
        .unwrap();
    assert!(prepared.is_installable());
    let outcome = execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(
        outcome.changed,
        vec!["athenos 0.9.0 → 1.0.0", "athenos-web 0.9.0 → 1.0.0"]
    );
    let state = world.state();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.0.0");
    assert!(state.previous.is_none());
    let first_backup = state.backup.clone().unwrap();
    assert!(world
        .host
        .log()
        .contains(&"rolling athenos-web".to_string()));

    // 2. Range: the upgrade brings a migration, applied before the start.
    let prepared = prepare_distribution(&depot, &range("athenos", "1.*"), &quiet)
        .await
        .unwrap();
    let athenos = prepared
        .plan
        .entries
        .iter()
        .find(|e| e.module == "athenos")
        .unwrap();
    assert_eq!(athenos.migrations, vec!["0002_links.sql"]);
    world.host.log.lock().unwrap().clear();
    let outcome = execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(outcome.migrations, vec!["athenos:migrations"]);
    let log = world.host.log();
    let migrate = log.iter().position(|l| l == "migrate athenos").unwrap();
    let start = log.iter().position(|l| l.starts_with("start ")).unwrap();
    assert!(migrate < start, "{log:?}");
    let state = world.state();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.0");
    assert_eq!(state.previous.as_ref().unwrap().id, "athenos-1.0.0");
    assert_ne!(state.backup.as_deref(), Some(first_backup.as_str()));
    assert!(
        !world.store.backup_dir(&first_backup).exists(),
        "older backups pruned"
    );

    // 3. Pin the web module; a hotfix of the core comes from a newer
    //    distribution and is recorded as an override.
    update_state(&world.store, |state| {
        state.pins.insert("athenos-web".to_string(), v("1.1.0"));
        Ok(())
    })
    .unwrap();
    let a111 = plain("athenos", "1.1.1");
    let w112 = plain("athenos-web", "1.1.2");
    world.release("athenos", "1.1.2", Channel::Stable, &[&a111, &w112]);
    let prepared = prepare_module(
        &depot,
        &name("athenos"),
        &"1.1.1".parse::<VersionPattern>().unwrap(),
        &quiet,
    )
    .await
    .unwrap();
    assert!(
        matches!(&prepared.intent, Intent::Module { candidate } if candidate.from.to_string() == "athenos-1.1.2")
    );
    execute(&depot, &prepared, &quiet).await.unwrap();
    let state = world.state();
    assert_eq!(state.overrides["athenos"].version, v("1.1.1"));
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.0");
    let newer = newer_distribution(
        &world.depot,
        &world.policy,
        &"athenos-1.1.0".parse().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(newer.id().to_string(), "athenos-1.1.2");

    // 4. athenos-1.1.2: the core is already its version (the override ends),
    //    the pinned web module stays; nothing changes on disk.
    let prepared = prepare_distribution(&depot, &exact("athenos-1.1.2"), &quiet)
        .await
        .unwrap();
    let web = prepared
        .plan
        .entries
        .iter()
        .find(|e| e.module == "athenos-web")
        .unwrap();
    assert_eq!(web.action, Action::Pinned);
    assert!(!prepared.plan.has_changes());
    execute(&depot, &prepared, &quiet).await.unwrap();
    let state = world.state();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.2");
    assert!(state.overrides.is_empty());
    assert!(state.backup.is_none());
    assert_eq!(world.host.version("athenos-web").as_deref(), Some("1.1.0"));

    // 5. Rollback without a local backup: athenos-1.1.0 again, verified.
    let prepared = prepare_rollback(&depot, &quiet).await.unwrap();
    assert!(matches!(
        prepared.intent,
        Intent::Distribution { rollback: true, .. }
    ));
    assert_eq!(
        prepared.plan.title,
        "rollback athenos-1.1.2 → athenos-1.1.0"
    );
    execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(world.host.version("athenos").as_deref(), Some("1.1.0"));
    assert_eq!(world.host.version("athenos-web").as_deref(), Some("1.1.0"));
    let state = world.state();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.0");
    assert_eq!(state.previous.as_ref().unwrap().id, "athenos-1.1.2");

    // 6. Rollback with the local backup of that downgrade: offline.
    *world.depot.details.lock().unwrap() = BTreeMap::new();
    let prepared = prepare_rollback(&depot, &quiet).await.unwrap();
    assert!(matches!(prepared.intent, Intent::Restore { .. }));
    let restore = &prepared.plan.entries[0];
    assert_eq!(
        (restore.module.as_str(), restore.action.clone()),
        ("athenos", Action::Restore)
    );
    execute(&depot, &prepared, &quiet).await.unwrap();
    assert_eq!(world.host.version("athenos").as_deref(), Some("1.1.1"));
    let state = world.state();
    assert_eq!(state.current.as_ref().unwrap().id, "athenos-1.1.2");
    assert!(state.previous.is_none() && state.backup.is_none());
    let err = prepare_rollback(&depot, &quiet).await.unwrap_err();
    assert!(matches!(err, DepotError::Refused(_)), "{err}");
}

#[tokio::test]
async fn an_unhealthy_module_rolls_the_whole_install_back() {
    let world = World::new(FakeHost::with(&[
        ("athenos", "1.0.0", 1, &[]),
        ("athenos-web", "1.0.0", 1, &[]),
    ]));
    let a = plain("athenos", "1.1.0");
    let w = plain("athenos-web", "1.1.0");
    world.release("athenos", "1.1.0", Channel::Stable, &[&a, &w]);
    world
        .host
        .unhealthy
        .lock()
        .unwrap()
        .insert("athenos-web@1.1.0".to_string());
    let depot = world.depot();
    let prepared = prepare_distribution(&depot, &exact("athenos-1.1.0"), &quiet)
        .await
        .unwrap();
    let err = execute(&depot, &prepared, &quiet).await.unwrap_err();
    assert!(
        matches!(&err, DepotError::Install { rolled_back: true, message } if message.contains("athenos-web 1.1.0 health check failed")),
        "{err}"
    );
    assert_eq!(world.host.version("athenos").as_deref(), Some("1.0.0"));
    assert_eq!(world.host.version("athenos-web").as_deref(), Some("1.0.0"));
    let state = world.state();
    assert!(state.current.is_none(), "nothing recorded");
    assert!(!world.store.root().join("lock").exists(), "lock released");
}

#[tokio::test]
async fn a_fresh_module_is_removed_again_when_the_install_fails() {
    let world = World::new(FakeHost::with(&[("athenos", "1.0.0", 1, &[])]));
    let a = plain("athenos", "1.1.0");
    let n = plain("notification-service", "6.2.0");
    world.release("athenos", "1.1.0", Channel::Stable, &[&a, &n]);
    world
        .host
        .unhealthy
        .lock()
        .unwrap()
        .insert("athenos@1.1.0".to_string());
    let depot = world.depot();
    let prepared = prepare_distribution(&depot, &exact("athenos-1.1.0"), &quiet)
        .await
        .unwrap();
    assert!(execute(&depot, &prepared, &quiet).await.is_err());
    assert_eq!(world.host.version("notification-service"), None);
    assert!(world
        .host
        .log()
        .contains(&"remove notification-service".to_string()));
}

#[tokio::test]
async fn a_second_operation_waits_for_the_lock_and_a_tampered_cache_is_noticed() {
    let world = World::new(FakeHost::with(&[("athenos", "1.0.0", 1, &[])]));
    let a = plain("athenos", "1.1.0");
    world.release("athenos", "1.1.0", Channel::Stable, &[&a]);
    let depot = world.depot();
    let prepared = prepare_distribution(&depot, &exact("athenos-1.1.0"), &quiet)
        .await
        .unwrap();

    std::fs::write(world.store.root().join("lock"), b"").unwrap();
    let err = execute(&depot, &prepared, &quiet).await.unwrap_err();
    assert!(err.to_string().contains("another depot operation"), "{err}");
    std::fs::remove_file(world.store.root().join("lock")).unwrap();

    // The verified cache file changes between plan and execution.
    let cached = BlobCache::new(world.store.blob_dir()).path(&a.module.sha256);
    std::fs::write(&cached, b"swapped on disk").unwrap();
    let err = execute(&depot, &prepared, &quiet).await.unwrap_err();
    assert!(
        matches!(&err, DepotError::Bundle(m) if m.contains("bytes, the signed manifest says")),
        "{err}"
    );
    assert!(
        world.host.log().is_empty(),
        "checked before anything changed"
    );
    assert_eq!(world.host.version("athenos").as_deref(), Some("1.0.0"));
}

#[test]
fn the_state_file_round_trips_and_refuses_another_format() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());
    assert_eq!(store.load().unwrap(), HostState::default());
    let mut state = HostState::default();
    state.pins.insert("athenos-web".to_string(), v("0.12.2"));
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), state);
    assert!(!dir.path().join("state.json.tmp").exists());
    std::fs::write(
        store.state_path(),
        br#"{"format":"other","current":null,"previous":null}"#,
    )
    .unwrap();
    assert!(store.load().is_err());
}

#[test]
fn blob_writer_checks_size_and_digest() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("blob");
    let digest = Sha256Digest::of(b"hello");
    let mut writer = BlobWriter::create(&target, digest, 5).unwrap();
    writer.write(b"hel").unwrap();
    writer.write(b"lo").unwrap();
    writer.finish().unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"hello");

    let other = dir.path().join("other");
    let mut writer = BlobWriter::create(&other, digest, 5).unwrap();
    assert!(writer.write(b"hello!").is_err(), "longer than signed");
    drop(writer);
    assert!(!other.exists() && !dir.path().join("other.part").exists());

    let mut writer = BlobWriter::create(&other, digest, 5).unwrap();
    writer.write(b"jello").unwrap();
    assert!(writer
        .finish()
        .unwrap_err()
        .to_string()
        .contains("SHA-256 mismatch"));
    assert!(!other.exists());
}
