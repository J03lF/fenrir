//! What the operator sees: the plan of `install distribution` and the
//! output of `status distribution` (run with `--nocapture` to print them).

use std::collections::BTreeMap;

use depot_protocol::manifest::MANIFEST_FORMAT;
use depot_protocol::verify::CheckedDistribution;
use depot_protocol::{Channel, Manifest, Sha256Digest, Timestamp, TrustedKeys};
use semver::Version;

use super::*;
use crate::services::depot::plan::{Action, Plan, PlanEntry};
use crate::services::depot::state::{InstalledDistribution, ModuleOverride, ModuleRecord};
use crate::services::depot::verify::VerifiedDistribution;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn policy() -> HostPolicy {
    HostPolicy {
        trusted: TrustedKeys::default(),
        channel: Channel::Stable,
        platform: "linux-x86_64".parse().unwrap(),
        fenrir: v("0.13.0"),
    }
}

fn entry(
    module: &str,
    rule: &str,
    installed: Option<&str>,
    target: Option<&str>,
    size: Option<u64>,
    action: Action,
    restart: Restart,
) -> PlanEntry {
    PlanEntry {
        module: module.to_string(),
        rule: Some(rule.to_string()),
        installed: installed.map(v),
        target: target.map(v),
        size,
        action,
        restart,
        migrations: Vec::new(),
        via: None,
        note: None,
        source: None,
    }
}

fn record(version: &str, sha: &str) -> ModuleRecord {
    ModuleRecord {
        version: v(version),
        sha256: sha.to_string(),
        size: 1,
        via: None,
    }
}

fn installed_state() -> HostState {
    HostState {
        current: Some(InstalledDistribution {
            id: "athenos-0.12.5".to_string(),
            manifest_sha256: "8670c3e18f3718511c27d91e652686f658db0009b15e59e231dc5137c80e3008"
                .to_string(),
            channel: "stable".to_string(),
            key_id: "34750f98bd59fcfc".to_string(),
            installed_at: "2026-10-29T14:02:11Z".to_string(),
            modules: BTreeMap::from([
                ("athenos".to_string(), record("0.12.5", "a")),
                ("athenos-web".to_string(), record("0.12.5", "w")),
                ("auth-service".to_string(), record("3.1.0", "s")),
            ]),
        }),
        ..HostState::default()
    }
}

#[test]
fn the_plan_shows_old_new_rule_size_restarts_migrations_and_pins() {
    let manifest = Manifest {
        format: MANIFEST_FORMAT.to_string(),
        product: "athenos".parse().unwrap(),
        version: v("0.13.0"),
        channel: Channel::Beta,
        platform: "linux-x86_64".parse().unwrap(),
        created_at: Timestamp::from_unix_seconds(1_800_000_000).unwrap(),
        modules: Vec::new(),
        includes: Vec::new(),
    };
    let mut athenos = entry(
        "athenos",
        "0.13.*",
        Some("0.12.5"),
        Some("0.13.0"),
        Some(53_208_112),
        Action::Update,
        Restart::Restart,
    );
    athenos.migrations = vec!["0002_ticket_links.sql".to_string()];
    let mut web = entry(
        "athenos-web",
        "0.13.*",
        Some("0.12.2"),
        Some("0.12.2"),
        None,
        Action::Pinned,
        Restart::None,
    );
    web.note = Some("distribution has 0.13.0".to_string());
    let mut notification = entry(
        "notification-service",
        "6.* || 7.*",
        None,
        Some("7.0.1"),
        Some(20_441_908),
        Action::Install,
        Restart::Start,
    );
    notification.via = Some("platform-0.5.0".to_string());
    let prepared = Prepared {
        intent: Intent::Distribution {
            distribution: Box::new(VerifiedDistribution {
                root: CheckedDistribution {
                    manifest,
                    digest: Sha256Digest::of(b"athenos-0.13.0"),
                    key_id: "34750f98bd59fcfc".parse().unwrap(),
                    channel: Channel::Stable,
                    yanked: None,
                },
                includes: Vec::new(),
            }),
            rollback: false,
        },
        plan: Plan {
            title: "athenos-0.13.0".to_string(),
            entries: vec![
                athenos,
                entry(
                    "athenos-api",
                    "0.13.*",
                    Some("0.12.4"),
                    Some("0.13.0"),
                    Some(24_100_000),
                    Action::Update,
                    Restart::Rolling,
                ),
                web,
                entry(
                    "auth-service",
                    "3.*",
                    Some("3.1.0"),
                    Some("3.1.0"),
                    None,
                    Action::Unchanged,
                    Restart::None,
                ),
                notification,
            ],
            problems: Vec::new(),
        },
        skipped: vec!["athenos-0.13.1: athenos-0.13.1 is yanked: broken migration".to_string()],
        host_includes: vec!["fenrir-0.13.0".to_string()],
        staged: BTreeMap::new(),
        downloaded: 53_208_112,
    };
    let mut out = Vec::new();
    render_plan(&mut out, &policy(), Some(&installed_state()), &prepared).unwrap();
    let text = String::from_utf8(out).unwrap();
    println!("{text}");
    for expected in [
        "Plan: athenos-0.13.0",
        "signed by key 34750f98bd59fcfc · channel stable",
        "installed now: athenos-0.12.5",
        "update · restart",
        "update · rolling restart",
        "pinned, kept (distribution has 0.13.0)",
        "notification-service (via platform-0.5.0)",
        "athenos 0002_ticket_links.sql",
        "fenrir-0.13.0 included (this host runs fenrir 0.13.0",
        "Skipped:    athenos-0.13.1",
        "Bundles:    3, 93.2 MB (50.7 MB downloaded now)",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
}

#[test]
fn the_status_shows_distribution_overrides_pins_previous_and_newer() {
    let mut state = installed_state();
    state.previous = Some(InstalledDistribution {
        id: "athenos-0.12.4".to_string(),
        ..state.current.clone().unwrap()
    });
    state.pins.insert("athenos-web".to_string(), v("0.12.2"));
    state.overrides.insert(
        "auth-service".to_string(),
        ModuleOverride {
            version: v("3.1.1"),
            sha256: "s2".to_string(),
            size: 1,
            from: "athenos-0.12.6".to_string(),
            installed_at: "2026-10-29T15:00:00Z".to_string(),
        },
    );
    let module = |name: &str, version: &str, sha: &str| HostModule {
        name: name.to_string(),
        version: v(version),
        sha256: sha.to_string(),
        local_sync: false,
        running: true,
        instances: 1,
        migrations: Vec::new(),
    };
    let view = StatusView {
        channel: Channel::Stable,
        url: "https://depot.athenos.dev:8443/".to_string(),
        state,
        backup_ok: true,
        depot: Ok((
            None,
            Some(("athenos-0.13.0".to_string(), "stable".to_string())),
        )),
        modules: vec![
            module("athenos", "0.12.5", "a"),
            module("athenos-web", "0.12.2", "w2"),
            module("auth-service", "3.1.1", "s2"),
            module("legacy", "0.1.0", "l"),
        ],
    };
    let mut out = Vec::new();
    render_status(&mut out, &view).unwrap();
    let text = String::from_utf8(out).unwrap();
    println!("{text}");
    for expected in [
        "Installed     athenos-0.12.5 (channel stable, installed 2026-10-29T14:02:11Z)",
        "Follows       stable (depot https://depot.athenos.dev:8443/)",
        "Overrides     auth-service 3.1.1 (signed in athenos-0.12.6)",
        "Pins          athenos-web 0.12.2",
        "Previous      athenos-0.12.4 (rollback: local backup)",
        "Newer         athenos-0.13.0 (stable) → install distribution athenos-0.13.0",
        "pinned at 0.12.2",
        "override 3.1.1",
        "not in distribution",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
}
