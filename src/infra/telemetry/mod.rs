use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::config::AppConfig;

static TELEMETRY: OnceLock<TelemetryState> = OnceLock::new();

struct TelemetryState {
    ready: AtomicBool,
    live: AtomicBool,
    start_time: Instant,
    config: TelemetryConfig,
    metrics: Mutex<HashMap<String, u64>>,
    readiness_probes: Mutex<Vec<Probe>>, // lazily evaluated health probes
}

impl TelemetryState {
    fn new(config: TelemetryConfig) -> Self {
        Self {
            ready: AtomicBool::new(false),
            live: AtomicBool::new(true),
            start_time: Instant::now(),
            config,
            metrics: Mutex::new(HashMap::new()),
            readiness_probes: Mutex::new(Vec::new()),
        }
    }
}

struct Probe {
    name: String,
    check: Box<dyn Fn() -> bool + Send + Sync + 'static>,
}

#[derive(Clone)]
struct TelemetryConfig {
    metrics_enabled: bool,
    health_enabled: bool,
}

impl From<&AppConfig> for TelemetryConfig {
    fn from(cfg: &AppConfig) -> Self {
        Self {
            metrics_enabled: cfg.telemetry.metrics_enabled,
            health_enabled: cfg.telemetry.health_enabled,
        }
    }
}

pub fn init(cfg: &AppConfig) -> Result<()> {
    let state = TelemetryState::new(TelemetryConfig::from(cfg));
    TELEMETRY
        .set(state)
        .map_err(|_| anyhow!("telemetry bereits initialisiert"))
}

pub fn mark_ready() {
    if let Some(state) = TELEMETRY.get() {
        state.ready.store(true, Ordering::Release);
    }
}

pub fn mark_not_ready() {
    if let Some(state) = TELEMETRY.get() {
        state.ready.store(false, Ordering::Release);
    }
}

pub fn mark_live(live: bool) {
    if let Some(state) = TELEMETRY.get() {
        state.live.store(live, Ordering::Release);
    }
}

pub fn register_readiness_probe<F>(name: impl Into<String>, probe: F) -> Result<()>
where
    F: Fn() -> bool + Send + Sync + 'static,
{
    let state = TELEMETRY
        .get()
        .ok_or_else(|| anyhow!("telemetry wurde noch nicht initialisiert"))?;
    let mut probes = state
        .readiness_probes
        .lock()
        .map_err(|_| anyhow!("readiness probes lock poisoned"))?;
    probes.push(Probe {
        name: name.into(),
        check: Box::new(probe),
    });
    Ok(())
}

pub fn record_counter(name: &str, delta: u64) {
    if let Some(state) = TELEMETRY.get() {
        if !state.config.metrics_enabled {
            return;
        }
        if let Ok(mut metrics) = state.metrics.lock() {
            let counter = metrics.entry(name.to_string()).or_insert(0);
            *counter = counter.saturating_add(delta);
        }
    }
}

pub fn set_counter(name: &str, value: u64) {
    if let Some(state) = TELEMETRY.get() {
        if !state.config.metrics_enabled {
            return;
        }
        if let Ok(mut metrics) = state.metrics.lock() {
            metrics.insert(name.to_string(), value);
        }
    }
}

pub fn is_ready() -> bool {
    if let Some(state) = TELEMETRY.get() {
        if !state.ready.load(Ordering::Acquire) {
            return false;
        }
        if !state.config.health_enabled {
            return true;
        }
        if let Ok(probes) = state.readiness_probes.lock() {
            for probe in probes.iter() {
                if !(probe.check)() {
                    tracing::warn!(probe = %probe.name, "readiness probe failed");
                    return false;
                }
            }
        }
        true
    } else {
        false
    }
}

pub fn is_live() -> bool {
    TELEMETRY
        .get()
        .map(|state| state.live.load(Ordering::Acquire))
        .unwrap_or(false)
}

pub fn uptime() -> Option<Duration> {
    TELEMETRY.get().map(|state| state.start_time.elapsed())
}

pub fn snapshot() -> Option<TelemetrySnapshot> {
    let state = TELEMETRY.get()?;
    let metrics = state.metrics.lock().ok()?.clone();
    Some(TelemetrySnapshot {
        ready: state.ready.load(Ordering::Acquire),
        live: state.live.load(Ordering::Acquire),
        uptime: state.start_time.elapsed(),
        metrics,
    })
}

#[derive(Clone, Debug)]
pub struct TelemetrySnapshot {
    pub ready: bool,
    pub live: bool,
    pub uptime: Duration,
    pub metrics: HashMap<String, u64>,
}
