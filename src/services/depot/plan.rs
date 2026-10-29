//! What an install would change, computed from the signed target set, the
//! installed modules and the host state (pins, overrides). Pure: no IO, so
//! every rule is unit-tested.

use std::collections::BTreeMap;

use semver::Version;

use super::ports::HostModule;
use super::state::{BackupRecord, HostState};
use super::verify::TargetModule;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Downgrade,
    /// Same version, other bytes (for example a module copied by rsync
    /// before the depot): replaced by the signed bundle.
    Replace,
    Unchanged,
    /// Kept as installed (`pin module`).
    Pinned,
    /// Synchronized from local sources: the depot does not touch it.
    LocalSync,
    /// An optional module that is not installed here.
    OptionalSkipped,
    /// Installed, but not part of the target: left alone.
    Kept,
    /// Rollback: back to the version in the backup.
    Restore,
    /// Rollback: the module was not there before.
    Remove,
}

impl Action {
    /// Whether files change.
    pub fn changes(&self) -> bool {
        matches!(
            self,
            Self::Install
                | Self::Update
                | Self::Downgrade
                | Self::Replace
                | Self::Restore
                | Self::Remove
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Downgrade => "downgrade",
            Self::Replace => "replace (other build)",
            Self::Unchanged => "unchanged",
            Self::Pinned => "pinned, kept",
            Self::LocalSync => "local sync, kept",
            Self::OptionalSkipped => "optional, not installed",
            Self::Kept => "not in distribution, kept",
            Self::Restore => "restore",
            Self::Remove => "remove",
        }
    }
}

/// How the module gets onto the new files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    None,
    /// Not running before: started.
    Start,
    /// One instance: stopped, replaced, started.
    Restart,
    /// Several instances: replaced one by one under health checks.
    Rolling,
    /// Stopped and uninstalled.
    Stop,
}

impl Restart {
    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::Start => "start",
            Self::Restart => "restart",
            Self::Rolling => "rolling restart",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub module: String,
    pub rule: Option<String>,
    pub installed: Option<Version>,
    /// What runs afterwards (`None` for a removal).
    pub target: Option<Version>,
    /// Bundle size of a change from the depot.
    pub size: Option<u64>,
    pub action: Action,
    pub restart: Restart,
    /// New migration files the bundle brings (filled in once the bundle is
    /// downloaded and verified).
    pub migrations: Vec<String>,
    pub via: Option<String>,
    pub note: Option<String>,
    /// The signed module this entry installs.
    pub source: Option<TargetModule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// `athenos-0.13.0`, `module notification-service 7.0.2`, `rollback`.
    pub title: String,
    pub entries: Vec<PlanEntry>,
    /// Reasons not to go ahead; the plan is shown, nothing is installed.
    pub problems: Vec<String>,
}

impl Plan {
    pub fn changes(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries.iter().filter(|entry| entry.action.changes())
    }

    pub fn has_changes(&self) -> bool {
        self.changes().next().is_some()
    }

    pub fn download_size(&self) -> u64 {
        self.changes()
            .filter_map(|entry| entry.size)
            .fold(0, u64::saturating_add)
    }
}

fn restart_for(installed: Option<&HostModule>) -> Restart {
    match installed {
        Some(module) if module.running && module.instances > 1 => Restart::Rolling,
        Some(module) if module.running => Restart::Restart,
        _ => Restart::Start,
    }
}

fn change_action(installed: Option<&HostModule>, target: &TargetModule) -> Action {
    match installed {
        None => Action::Install,
        Some(module) if module.version == target.version => {
            if module.sha256.eq_ignore_ascii_case(&target.sha256.to_hex()) {
                Action::Unchanged
            } else {
                Action::Replace
            }
        }
        Some(module) if module.version < target.version => Action::Update,
        Some(_) => Action::Downgrade,
    }
}

/// `install distribution`: every target module against what is installed.
pub fn plan_distribution(
    title: &str,
    targets: &[TargetModule],
    installed: &[HostModule],
    state: &HostState,
) -> Plan {
    let by_name: BTreeMap<&str, &HostModule> = installed
        .iter()
        .map(|module| (module.name.as_str(), module))
        .collect();
    let mut entries = Vec::new();
    for target in targets {
        let current = by_name.get(target.name.as_str()).copied();
        let mut note = None;
        let action = if current.is_some_and(|module| module.local_sync) {
            Action::LocalSync
        } else if let Some(pinned) = state.pins.get(&target.name) {
            if pinned != &target.version {
                note = Some(format!("distribution has {}", target.version));
            }
            Action::Pinned
        } else if target.optional && current.is_none() {
            Action::OptionalSkipped
        } else {
            change_action(current, target)
        };
        if action.changes() || action == Action::Unchanged {
            if let Some(over) = state.overrides.get(&target.name) {
                note = Some(format!("ends override {} ({})", over.version, over.from));
            }
        }
        let runs = match action {
            Action::Pinned | Action::LocalSync => current.map(|module| module.version.clone()),
            Action::OptionalSkipped => None,
            _ => Some(target.version.clone()),
        };
        entries.push(PlanEntry {
            module: target.name.clone(),
            rule: Some(target.rule.clone()),
            installed: current.map(|module| module.version.clone()),
            target: runs,
            size: action.changes().then_some(target.size),
            restart: if action.changes() {
                restart_for(current)
            } else {
                Restart::None
            },
            action,
            migrations: Vec::new(),
            via: target.via.clone(),
            note,
            source: Some(target.clone()),
        });
    }
    for module in installed {
        if targets.iter().any(|target| target.name == module.name) {
            continue;
        }
        entries.push(PlanEntry {
            module: module.name.clone(),
            rule: None,
            installed: Some(module.version.clone()),
            target: Some(module.version.clone()),
            size: None,
            action: if module.local_sync {
                Action::LocalSync
            } else {
                Action::Kept
            },
            restart: Restart::None,
            migrations: Vec::new(),
            via: None,
            note: None,
            source: None,
        });
    }
    entries.sort_by(|a, b| a.module.cmp(&b.module));
    let problems = effective_requirements(&entries);
    Plan {
        title: title.to_string(),
        entries,
        problems,
    }
}

/// Requirements of the signed modules against what will actually run.
/// The signed set was checked as a whole already; this only catches what
/// the host changes about it (pins, local sync, optional modules left out).
fn effective_requirements(entries: &[PlanEntry]) -> Vec<String> {
    let runs: BTreeMap<&str, (&Version, &Action)> = entries
        .iter()
        .filter_map(|entry| {
            entry
                .target
                .as_ref()
                .map(|version| (entry.module.as_str(), (version, &entry.action)))
        })
        .collect();
    let mut problems = Vec::new();
    for entry in entries {
        let Some(source) = &entry.source else {
            continue;
        };
        if !(entry.action.changes() || entry.action == Action::Unchanged) {
            continue;
        }
        for requirement in &source.requires.modules {
            let needed = requirement.module.as_str();
            match runs.get(needed) {
                Some((version, action)) if !requirement.rule.matches(version) => {
                    let why = match action {
                        Action::Pinned => "pinned at",
                        Action::LocalSync => "synchronized locally at",
                        Action::Kept => "kept at",
                        _ => "at",
                    };
                    problems.push(format!(
                        "{} {} requires {needed} {}, but {needed} stays {why} {version}",
                        source.name, source.version, requirement.rule
                    ));
                }
                Some(_) => {}
                None => problems.push(format!(
                    "{} {} requires {needed} {}, but {needed} will not be installed",
                    source.name, source.version, requirement.rule
                )),
            }
        }
    }
    problems
}

/// `install module`: one signed module on top of what runs.
pub fn plan_module(target: &TargetModule, installed: &[HostModule], state: &HostState) -> Plan {
    let current = installed.iter().find(|module| module.name == target.name);
    let mut problems = Vec::new();
    if let Some(pinned) = state.pins.get(&target.name) {
        problems.push(format!(
            "{} is pinned at {pinned}; `unpin module {}` first",
            target.name, target.name
        ));
    }
    if current.is_some_and(|module| module.local_sync) {
        problems.push(format!(
            "{} is synchronized from local sources; `release module {}` first",
            target.name, target.name
        ));
    }
    let action = change_action(current, target);
    for requirement in &target.requires.modules {
        let needed = requirement.module.as_str();
        match installed.iter().find(|module| module.name == needed) {
            Some(module) if !requirement.rule.matches(&module.version) => problems.push(format!(
                "{} {} requires {needed} {}, but {needed} {} is installed",
                target.name, target.version, requirement.rule, module.version
            )),
            Some(_) => {}
            None => problems.push(format!(
                "{} {} requires {needed} {}, which is not installed",
                target.name, target.version, requirement.rule
            )),
        }
    }
    let entry = PlanEntry {
        module: target.name.clone(),
        rule: None,
        installed: current.map(|module| module.version.clone()),
        target: Some(target.version.clone()),
        size: action.changes().then_some(target.size),
        restart: if action.changes() {
            restart_for(current)
        } else {
            Restart::None
        },
        action,
        migrations: Vec::new(),
        via: None,
        note: None,
        source: Some(target.clone()),
    };
    Plan {
        title: format!("module {} {}", target.name, target.version),
        entries: vec![entry],
        problems,
    }
}

/// `rollback` from a local backup: every module of the backed-up change
/// back to its version from before, or removed if it was new.
pub fn plan_restore(title: &str, record: &BackupRecord, installed: &[HostModule]) -> Plan {
    let mut entries: Vec<PlanEntry> = record
        .modules
        .iter()
        .map(|backed| {
            let current = installed.iter().find(|module| module.name == backed.module);
            let (action, target, restart) = match &backed.previous {
                Some(previous) => (
                    Action::Restore,
                    Some(previous.version.clone()),
                    restart_for(current),
                ),
                None => (
                    Action::Remove,
                    None,
                    if current.is_some() {
                        Restart::Stop
                    } else {
                        Restart::None
                    },
                ),
            };
            PlanEntry {
                module: backed.module.clone(),
                rule: None,
                installed: current.map(|module| module.version.clone()),
                target,
                size: None,
                action,
                restart,
                migrations: Vec::new(),
                via: None,
                note: Some("from the local backup".to_string()),
                source: None,
            }
        })
        .collect();
    entries.sort_by(|a, b| a.module.cmp(&b.module));
    Plan {
        title: title.to_string(),
        entries,
        problems: Vec::new(),
    }
}
