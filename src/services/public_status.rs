use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use tokio::sync::broadcast::error::RecvError;

use super::{ServiceRegistry, ServiceSnapshot, ServiceStatus};

const DEFAULT_MAX_RECENT_INCIDENTS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PublicComponentKey {
    Website,
    Account,
    Api,
    Notifications,
}

impl PublicComponentKey {
    pub const REQUIRED: [PublicComponentKey; 3] = [
        PublicComponentKey::Website,
        PublicComponentKey::Account,
        PublicComponentKey::Api,
    ];

    pub fn as_key(self) -> &'static str {
        match self {
            PublicComponentKey::Website => "website",
            PublicComponentKey::Account => "account",
            PublicComponentKey::Api => "api",
            PublicComponentKey::Notifications => "notifications",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            PublicComponentKey::Website => "Website",
            PublicComponentKey::Account => "Account & Login",
            PublicComponentKey::Api => "API",
            PublicComponentKey::Notifications => "Notifications",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PublicComponentStatus {
    Unknown,
    Up,
    Degraded,
    Down,
}

impl PublicComponentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PublicComponentStatus::Unknown => "unknown",
            PublicComponentStatus::Up => "up",
            PublicComponentStatus::Degraded => "degraded",
            PublicComponentStatus::Down => "down",
        }
    }

    fn is_incident(self) -> bool {
        matches!(
            self,
            PublicComponentStatus::Degraded | PublicComponentStatus::Down
        )
    }

    fn severity(self) -> PublicIncidentSeverity {
        match self {
            PublicComponentStatus::Down => PublicIncidentSeverity::Major,
            PublicComponentStatus::Degraded => PublicIncidentSeverity::Minor,
            PublicComponentStatus::Unknown | PublicComponentStatus::Up => {
                PublicIncidentSeverity::Minor
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicIncidentSeverity {
    Minor,
    Major,
}

impl PublicIncidentSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            PublicIncidentSeverity::Minor => "minor",
            PublicIncidentSeverity::Major => "major",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicIncidentStatus {
    Open,
    Resolved,
}

impl PublicIncidentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PublicIncidentStatus::Open => "open",
            PublicIncidentStatus::Resolved => "resolved",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublicIncidentSnapshot {
    pub id: String,
    pub component: PublicComponentKey,
    pub title: String,
    pub status: PublicIncidentStatus,
    pub severity: PublicIncidentSeverity,
    pub started_at: SystemTime,
    pub resolved_at: Option<SystemTime>,
}

#[derive(Clone, Debug)]
struct PublicIncidentRecord {
    id: String,
    component: PublicComponentKey,
    title: String,
    severity: PublicIncidentSeverity,
    started_at: SystemTime,
    resolved_at: Option<SystemTime>,
}

impl PublicIncidentRecord {
    fn open(component: PublicComponentKey, status: PublicComponentStatus, sequence: u64) -> Self {
        Self {
            id: format!("inc-{sequence:06}"),
            component,
            title: incident_title(component, status),
            severity: status.severity(),
            started_at: SystemTime::now(),
            resolved_at: None,
        }
    }

    fn snapshot(&self, status: PublicIncidentStatus) -> PublicIncidentSnapshot {
        PublicIncidentSnapshot {
            id: self.id.clone(),
            component: self.component,
            title: self.title.clone(),
            status,
            severity: self.severity,
            started_at: self.started_at,
            resolved_at: self.resolved_at,
        }
    }
}

#[derive(Default)]
struct PublicStatusTrackerState {
    sequence: u64,
    open_incidents: HashMap<PublicComponentKey, PublicIncidentRecord>,
    resolved_incidents: VecDeque<PublicIncidentRecord>,
    component_levels: HashMap<PublicComponentKey, PublicComponentStatus>,
    service_levels: HashMap<String, PublicComponentStatus>,
}

pub struct PublicStatusTracker {
    started: AtomicBool,
    max_recent_incidents: usize,
    state: Arc<RwLock<PublicStatusTrackerState>>,
}

impl Default for PublicStatusTracker {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RECENT_INCIDENTS)
    }
}

impl PublicStatusTracker {
    pub fn new(max_recent_incidents: usize) -> Self {
        Self {
            started: AtomicBool::new(false),
            max_recent_incidents,
            state: Arc::new(RwLock::new(PublicStatusTrackerState::default())),
        }
    }

    pub fn start(&self, registry: Arc<ServiceRegistry>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut state) = self.state.write() {
            seed_levels_from_snapshot(&mut state, registry.snapshot());
        }
        let state = Arc::clone(&self.state);
        let max_recent = self.max_recent_incidents;
        tokio::spawn(async move {
            let mut events = registry.subscribe();
            loop {
                match events.recv().await {
                    Ok(snapshot) => {
                        if let Ok(mut guard) = state.write() {
                            apply_transition(&mut guard, snapshot, max_recent);
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    pub fn recent_incidents(&self, limit: usize) -> Vec<PublicIncidentSnapshot> {
        let guard = match self.state.read() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };
        let mut incidents: Vec<PublicIncidentSnapshot> = guard
            .open_incidents
            .values()
            .map(|record| record.snapshot(PublicIncidentStatus::Open))
            .collect();
        incidents.extend(
            guard
                .resolved_incidents
                .iter()
                .map(|record| record.snapshot(PublicIncidentStatus::Resolved)),
        );
        incidents.sort_by(|a, b| {
            let a_time = a.resolved_at.or(Some(a.started_at));
            let b_time = b.resolved_at.or(Some(b.started_at));
            b_time.cmp(&a_time)
        });
        incidents.into_iter().take(limit).collect()
    }
}

pub fn map_service_to_public_component(id: &str) -> Option<PublicComponentKey> {
    if id == "module:athene-web::static" || id.starts_with("module:athene-web::") {
        return Some(PublicComponentKey::Website);
    }
    if id == "module:athene-api::api-gateway" || id.starts_with("module:athene-api::") {
        return Some(PublicComponentKey::Api);
    }
    if id.starts_with("module:auth-service::") || id == "identity-service" {
        return Some(PublicComponentKey::Account);
    }
    if id.starts_with("module:notification-service::") {
        return Some(PublicComponentKey::Notifications);
    }
    None
}

fn seed_levels_from_snapshot(state: &mut PublicStatusTrackerState, snapshot: Vec<ServiceSnapshot>) {
    state.service_levels.clear();
    state.component_levels.clear();
    for service in snapshot {
        let Some(component) = map_service_to_public_component(&service.descriptor.id) else {
            continue;
        };
        let status = status_from_service_status(service.status);
        state
            .service_levels
            .insert(service.descriptor.id.clone(), status);
        state
            .component_levels
            .entry(component)
            .and_modify(|value| *value = (*value).max(status))
            .or_insert(status);
    }
}

fn apply_transition(
    state: &mut PublicStatusTrackerState,
    snapshot: ServiceSnapshot,
    max_recent_incidents: usize,
) {
    let Some(component) = map_service_to_public_component(&snapshot.descriptor.id) else {
        return;
    };
    let previous = state
        .component_levels
        .get(&component)
        .copied()
        .unwrap_or(PublicComponentStatus::Unknown);

    state.service_levels.insert(
        snapshot.descriptor.id.clone(),
        status_from_service_status(snapshot.status),
    );
    let current = recompute_component_status(component, &state.service_levels);
    state.component_levels.insert(component, current);

    if !previous.is_incident() && current.is_incident() {
        state.sequence = state.sequence.saturating_add(1);
        let incident = PublicIncidentRecord::open(component, current, state.sequence);
        state.open_incidents.insert(component, incident);
        return;
    }
    if previous.is_incident() && current.is_incident() {
        if let Some(open) = state.open_incidents.get_mut(&component) {
            open.severity = current.severity();
            open.title = incident_title(component, current);
        }
        return;
    }
    if previous.is_incident() && !current.is_incident() {
        if let Some(mut resolved) = state.open_incidents.remove(&component) {
            resolved.resolved_at = Some(SystemTime::now());
            state.resolved_incidents.push_front(resolved);
            while state.resolved_incidents.len() > max_recent_incidents {
                let _ = state.resolved_incidents.pop_back();
            }
        }
    }
}

fn recompute_component_status(
    component: PublicComponentKey,
    service_levels: &HashMap<String, PublicComponentStatus>,
) -> PublicComponentStatus {
    service_levels
        .iter()
        .filter_map(|(id, level)| {
            if map_service_to_public_component(id) == Some(component) {
                Some(*level)
            } else {
                None
            }
        })
        .max()
        .unwrap_or(PublicComponentStatus::Unknown)
}

fn status_from_service_status(status: ServiceStatus) -> PublicComponentStatus {
    match status {
        ServiceStatus::Failed | ServiceStatus::Stopped => PublicComponentStatus::Down,
        ServiceStatus::Degraded | ServiceStatus::Starting => PublicComponentStatus::Degraded,
        ServiceStatus::Active => PublicComponentStatus::Up,
        ServiceStatus::Standby => PublicComponentStatus::Unknown,
    }
}

fn incident_title(component: PublicComponentKey, status: PublicComponentStatus) -> String {
    match status {
        PublicComponentStatus::Down => format!("{} outage", component.display_name()),
        PublicComponentStatus::Degraded => format!("{} degraded", component.display_name()),
        PublicComponentStatus::Unknown | PublicComponentStatus::Up => {
            format!("{} recovered", component.display_name())
        }
    }
}
