use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

const MAX_LATENCY_SAMPLES: usize = 64;
pub const DEFAULT_STALE_AFTER_SECS: u64 = 180;

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
}

impl ServiceDiagnostics {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
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
