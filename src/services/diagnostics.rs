use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde_json::Value as JsonValue;

const MAX_LATENCY_SAMPLES: usize = 64;
pub const DEFAULT_STALE_AFTER_SECS: u64 = 180;
pub const DEFAULT_RESOURCE_STALE_AFTER_SECS: u64 = 60;

#[derive(Default)]
struct ServiceMetricEntry {
    last_heartbeat: Option<SystemTime>,
    last_failure: Option<SystemTime>,
    last_success: Option<SystemTime>,
    latencies_ms: VecDeque<f64>,
    successes: u64,
    failures: u64,
}

impl ServiceMetricEntry {
    fn record_probe(&mut self, latency_ms: f64, success: bool) {
        self.last_heartbeat = Some(SystemTime::now());
        if !latency_ms.is_nan() && latency_ms.is_finite() {
            self.latencies_ms.push_back(latency_ms);
            if self.latencies_ms.len() > MAX_LATENCY_SAMPLES {
                self.latencies_ms.pop_front();
            }
        }
        if success {
            self.successes = self.successes.saturating_add(1);
            self.last_success = Some(SystemTime::now());
        } else {
            self.failures = self.failures.saturating_add(1);
            self.last_failure = Some(SystemTime::now());
        }
    }

    fn record_heartbeat(&mut self) {
        self.last_heartbeat = Some(SystemTime::now());
    }

    fn snapshot(&self) -> ServiceMetricSnapshot {
        let mut latencies: Vec<f64> = self.latencies_ms.iter().copied().collect();
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p50 = percentile(&latencies, 0.50);
        let p95 = percentile(&latencies, 0.95);
        let total = self.successes.saturating_add(self.failures);
        let error_rate = if total > 0 {
            Some((self.failures as f64 / total as f64) * 100.0)
        } else {
            None
        };
        ServiceMetricSnapshot {
            last_heartbeat: self.last_heartbeat,
            last_failure: self.last_failure,
            last_success: self.last_success,
            latency_p50_ms: p50,
            latency_p95_ms: p95,
            error_rate_pct: error_rate,
            sample_count: total,
        }
    }
}

fn percentile(samples: &[f64], percentile: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let pct = percentile.clamp(0.0, 1.0);
    let idx = ((samples.len() - 1) as f64 * pct).round() as usize;
    samples.get(idx).copied()
}

#[derive(Default)]
pub struct ServiceDiagnostics {
    entries: Mutex<HashMap<String, ServiceMetricEntry>>,
    runtime_metrics: ServiceRuntimeMetrics,
}

impl ServiceDiagnostics {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            runtime_metrics: ServiceRuntimeMetrics::new(),
        }
    }

    pub fn record_probe(&self, service_id: &str, latency_ms: f64, success: bool) {
        if let Ok(mut guard) = self.entries.lock() {
            let entry = guard.entry(service_id.to_string()).or_default();
            entry.record_probe(latency_ms, success);
        }
    }

    pub fn record_heartbeat(&self, service_id: &str) {
        if let Ok(mut guard) = self.entries.lock() {
            let entry = guard.entry(service_id.to_string()).or_default();
            entry.record_heartbeat();
        }
    }

    pub fn snapshot(&self, service_id: &str) -> Option<ServiceMetricSnapshot> {
        self.entries
            .lock()
            .ok()
            .and_then(|guard| guard.get(service_id).map(ServiceMetricEntry::snapshot))
    }

    pub fn snapshot_all(&self) -> HashMap<String, ServiceMetricSnapshot> {
        self.entries
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(id, entry)| (id.clone(), entry.snapshot()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn update_runtime_metrics(&self, service_id: &str, payload: JsonValue) {
        self.runtime_metrics.update(service_id, payload);
    }

    pub fn record_runtime_metrics_failure(&self, service_id: &str, error: impl Into<String>) {
        self.runtime_metrics.record_failure(service_id, error);
    }

    pub fn clear_runtime_metrics(&self, service_id: &str) {
        self.runtime_metrics.clear(service_id);
    }

    pub fn runtime_metrics_snapshot(
        &self,
        service_id: &str,
    ) -> Option<ServiceRuntimeMetricsSnapshot> {
        self.runtime_metrics.snapshot(service_id)
    }

    pub fn runtime_metrics_snapshot_all(&self) -> HashMap<String, ServiceRuntimeMetricsSnapshot> {
        self.runtime_metrics.snapshot_all()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceMetricSnapshot {
    pub last_heartbeat: Option<SystemTime>,
    pub last_failure: Option<SystemTime>,
    pub last_success: Option<SystemTime>,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub error_rate_pct: Option<f64>,
    pub sample_count: u64,
}

impl ServiceMetricSnapshot {
    pub fn last_heartbeat_elapsed(&self) -> Option<Duration> {
        self.last_heartbeat.and_then(|hb| hb.elapsed().ok())
    }

    pub fn last_failure_elapsed(&self) -> Option<Duration> {
        self.last_failure.and_then(|ts| ts.elapsed().ok())
    }

    pub fn last_success_elapsed(&self) -> Option<Duration> {
        self.last_success.and_then(|ts| ts.elapsed().ok())
    }

    pub fn is_stale(&self, threshold: Duration) -> bool {
        self.last_heartbeat_elapsed()
            .map(|elapsed| elapsed >= threshold)
            .unwrap_or(false)
    }
}

#[derive(Default)]
struct ServiceRuntimeMetricsEntry {
    updated_at: Option<SystemTime>,
    payload: Option<JsonValue>,
    last_error: Option<String>,
}

#[derive(Default)]
pub struct ServiceRuntimeMetrics {
    entries: Mutex<HashMap<String, ServiceRuntimeMetricsEntry>>,
}

impl ServiceRuntimeMetrics {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn update(&self, service_id: &str, payload: JsonValue) {
        if let Ok(mut guard) = self.entries.lock() {
            let entry = guard.entry(service_id.to_string()).or_default();
            entry.updated_at = Some(SystemTime::now());
            entry.payload = Some(payload);
            entry.last_error = None;
        }
    }

    pub fn record_failure(&self, service_id: &str, error: impl Into<String>) {
        if let Ok(mut guard) = self.entries.lock() {
            let entry = guard.entry(service_id.to_string()).or_default();
            entry.last_error = Some(error.into());
        }
    }

    pub fn clear(&self, service_id: &str) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.remove(service_id);
        }
    }

    pub fn snapshot(&self, service_id: &str) -> Option<ServiceRuntimeMetricsSnapshot> {
        self.entries.lock().ok().and_then(|guard| {
            guard
                .get(service_id)
                .map(ServiceRuntimeMetricsEntry::snapshot)
        })
    }

    pub fn snapshot_all(&self) -> HashMap<String, ServiceRuntimeMetricsSnapshot> {
        self.entries
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(id, entry)| (id.clone(), entry.snapshot()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl ServiceRuntimeMetricsEntry {
    fn snapshot(&self) -> ServiceRuntimeMetricsSnapshot {
        ServiceRuntimeMetricsSnapshot {
            updated_at: self.updated_at,
            payload: self.payload.clone(),
            last_error: self.last_error.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceRuntimeMetricsSnapshot {
    pub updated_at: Option<SystemTime>,
    pub payload: Option<JsonValue>,
    pub last_error: Option<String>,
}

impl ServiceRuntimeMetricsSnapshot {
    pub fn updated_at_elapsed(&self) -> Option<Duration> {
        self.updated_at.and_then(|ts| ts.elapsed().ok())
    }

    pub fn is_stale(&self, threshold: Duration) -> bool {
        self.updated_at_elapsed()
            .map(|elapsed| elapsed >= threshold)
            .unwrap_or(true)
    }
}
