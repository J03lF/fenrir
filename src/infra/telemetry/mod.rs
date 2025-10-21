use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{anyhow, Result};

#[cfg(target_os = "macos")]
use std::mem;

use crate::config::AppConfig;
use crate::services::{ServiceRegistry, ServiceSnapshot, ServiceStatus, ServiceTag};
use serde::{Deserialize, Serialize};
use serde_json;
use time::OffsetDateTime;

static TELEMETRY: OnceLock<TelemetryState> = OnceLock::new();
static SERVICE_METRICS_ATTACHED: AtomicBool = AtomicBool::new(false);
static SYSTEM_METRICS_STARTED: AtomicBool = AtomicBool::new(false);

const HISTORY_FILENAME: &str = "fenrir-telemetry-history.json";
const HISTORY_RETENTION_SECS: u64 = 24 * 60 * 60; // 24h
const HISTORY_PERSIST_INTERVAL_SECS: u64 = 30;
const HISTORY_MAX_SAMPLES: usize = 200_000;
const PROCESS_HISTORY_PREFIX: &str = "process.";
const HISTORY_VERSION: u8 = 1;

struct TelemetryState {
    ready: AtomicBool,
    live: AtomicBool,
    start_time: Instant,
    metrics_enabled: AtomicBool,
    health_enabled: AtomicBool,
    metrics: Mutex<HashMap<String, u64>>,
    readiness_probes: Mutex<Vec<Probe>>, // lazily evaluated health probes
    history: Mutex<VecDeque<MetricHistorySample>>,
    history_path: PathBuf,
    history_retention: Duration,
    history_persist_interval: Duration,
    last_persist: Mutex<Instant>,
}

impl TelemetryState {
    fn new(config: TelemetryConfig) -> Self {
        let history_path = telemetry_history_path();
        let history_retention = Duration::from_secs(HISTORY_RETENTION_SECS);
        let history_persist_interval = Duration::from_secs(HISTORY_PERSIST_INTERVAL_SECS);
        let history = load_history_from_disk(&history_path, history_retention);
        let initial_persist = Instant::now()
            .checked_sub(history_persist_interval)
            .unwrap_or_else(Instant::now);
        Self {
            ready: AtomicBool::new(false),
            live: AtomicBool::new(true),
            start_time: Instant::now(),
            metrics_enabled: AtomicBool::new(config.metrics_enabled),
            health_enabled: AtomicBool::new(config.health_enabled),
            metrics: Mutex::new(HashMap::new()),
            readiness_probes: Mutex::new(Vec::new()),
            history: Mutex::new(history),
            history_path,
            history_retention,
            history_persist_interval,
            last_persist: Mutex::new(initial_persist),
        }
    }

    fn push_history(&self, metrics: &HashMap<String, u64>) {
        let filtered = filter_history_metrics(metrics);
        if filtered.is_empty() {
            return;
        }
        let sample = MetricHistorySample {
            timestamp_ms: now_ms(),
            metrics: filtered,
        };
        let mut snapshot: Option<Vec<MetricHistorySample>> = None;
        if let Ok(mut history) = self.history.lock() {
            history.push_back(sample);
            self.trim_history(&mut history);
            if self.should_persist() {
                snapshot = Some(history.iter().cloned().collect());
            }
        }
        if let Some(samples) = snapshot {
            self.persist_history(samples);
        }
    }

    fn history(&self, range: Duration) -> Vec<MetricHistoryPoint> {
        let cutoff = now_ms().saturating_sub(duration_to_millis(range));
        if let Ok(history) = self.history.lock() {
            history
                .iter()
                .filter(|sample| sample.timestamp_ms >= cutoff)
                .cloned()
                .map(MetricHistoryPoint::from)
                .collect()
        } else {
            Vec::new()
        }
    }

    fn trim_history(&self, history: &mut VecDeque<MetricHistorySample>) {
        let cutoff = now_ms().saturating_sub(duration_to_millis(self.history_retention));
        while let Some(front) = history.front() {
            if front.timestamp_ms < cutoff {
                history.pop_front();
            } else {
                break;
            }
        }
        while history.len() > HISTORY_MAX_SAMPLES {
            history.pop_front();
        }
    }

    fn should_persist(&self) -> bool {
        if let Ok(mut guard) = self.last_persist.lock() {
            if guard.elapsed() >= self.history_persist_interval {
                *guard = Instant::now();
                return true;
            }
        }
        false
    }

    fn persist_history(&self, samples: Vec<MetricHistorySample>) {
        let payload = HistoryFile {
            version: HISTORY_VERSION,
            samples,
        };
        match serde_json::to_vec(&payload) {
            Ok(data) => {
                if let Some(parent) = self.history_path.parent() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        tracing::debug!(error = %err, path = ?parent, "telemetrie-history verzeichnis konnte nicht erstellt werden");
                        return;
                    }
                }
                if let Err(err) = fs::write(&self.history_path, data) {
                    tracing::debug!(error = %err, path = ?self.history_path, "telemetrie-history konnte nicht geschrieben werden");
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "telemetrie-history konnte nicht serialisiert werden");
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct MetricHistorySample {
    timestamp_ms: i64,
    metrics: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct MetricHistoryPoint {
    pub timestamp_ms: i64,
    pub metrics: HashMap<String, u64>,
}

impl From<MetricHistorySample> for MetricHistoryPoint {
    fn from(sample: MetricHistorySample) -> Self {
        Self {
            timestamp_ms: sample.timestamp_ms,
            metrics: sample.metrics,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    version: u8,
    samples: Vec<MetricHistorySample>,
}

fn telemetry_history_path() -> PathBuf {
    let base = telemetry_runtime_dir();
    base.join(HISTORY_FILENAME)
}

fn load_history_from_disk(path: &PathBuf, retention: Duration) -> VecDeque<MetricHistorySample> {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<HistoryFile>(&bytes) {
            Ok(file) => {
                let mut samples = file.samples;
                samples.sort_by_key(|sample| sample.timestamp_ms);
                let cutoff = now_ms().saturating_sub(duration_to_millis(retention));
                samples.retain(|sample| sample.timestamp_ms >= cutoff);
                if samples.len() > HISTORY_MAX_SAMPLES {
                    let start = samples.len().saturating_sub(HISTORY_MAX_SAMPLES);
                    samples = samples.split_off(start);
                }
                VecDeque::from(samples)
            }
            Err(err) => {
                tracing::debug!(error = %err, path = ?path, "telemetrie-history konnte nicht geparst werden");
                VecDeque::new()
            }
        },
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(error = %err, path = ?path, "telemetrie-history konnte nicht gelesen werden");
            }
            VecDeque::new()
        }
    }
}

fn filter_history_metrics(metrics: &HashMap<String, u64>) -> HashMap<String, u64> {
    metrics
        .iter()
        .filter_map(|(key, value)| {
            if key.starts_with(PROCESS_HISTORY_PREFIX) {
                Some((key.clone(), *value))
            } else {
                None
            }
        })
        .collect()
}

fn duration_to_millis(duration: Duration) -> i64 {
    let millis = duration.as_millis();
    if millis > i64::MAX as u128 {
        i64::MAX
    } else {
        millis as i64
    }
}

fn telemetry_runtime_dir() -> PathBuf {
    if let Ok(dir) = env::var("FENRIR_RUNTIME_DIR") {
        let path = PathBuf::from(dir);
        if ensure_dir(&path) {
            return path;
        }
    }
    let fallback = env::temp_dir().join("fenrir-runtime");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn ensure_dir(path: &Path) -> bool {
    fs::create_dir_all(path).is_ok()
}

fn now_ms() -> i64 {
    let now = OffsetDateTime::now_utc();
    (now.unix_timestamp_nanos() / 1_000_000) as i64
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

#[derive(Clone)]
struct ProcessMetricsConfig {
    enabled: bool,
    interval: Duration,
}

impl From<&AppConfig> for ProcessMetricsConfig {
    fn from(cfg: &AppConfig) -> Self {
        const DEFAULT_INTERVAL_MS: u64 = 5_000;
        const MIN_INTERVAL_MS: u64 = 500;
        let system = &cfg.telemetry.system;
        let interval_ms = system
            .interval_ms
            .unwrap_or(DEFAULT_INTERVAL_MS)
            .max(MIN_INTERVAL_MS);
        Self {
            enabled: system.enabled,
            interval: Duration::from_millis(interval_ms),
        }
    }
}

pub fn init(cfg: &AppConfig) -> Result<()> {
    let state = TelemetryState::new(TelemetryConfig::from(cfg));
    TELEMETRY
        .set(state)
        .map_err(|_| anyhow!("telemetry bereits initialisiert"))
}

pub fn reload(cfg: &AppConfig) {
    if let Some(state) = TELEMETRY.get() {
        let new_cfg = TelemetryConfig::from(cfg);
        state
            .metrics_enabled
            .store(new_cfg.metrics_enabled, Ordering::Release);
        state
            .health_enabled
            .store(new_cfg.health_enabled, Ordering::Release);
        tracing::info!(
            metrics_enabled = new_cfg.metrics_enabled,
            health_enabled = new_cfg.health_enabled,
            "telemetry-konfiguration aktualisiert"
        );
    }
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
        if !state.metrics_enabled.load(Ordering::Acquire) {
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
        if !state.metrics_enabled.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut metrics) = state.metrics.lock() {
            metrics.insert(name.to_string(), value);
        }
    }
}

fn record_process_history_sample() {
    if let Some(state) = TELEMETRY.get() {
        if !state.metrics_enabled.load(Ordering::Acquire) {
            return;
        }
        if let Ok(metrics) = state.metrics.lock() {
            state.push_history(&metrics);
        }
    }
}

pub fn attach_service_registry(registry: Arc<ServiceRegistry>) {
    if !SERVICE_METRICS_ATTACHED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        return;
    }

    update_service_counters(&registry.snapshot());

    let registry_for_task = Arc::clone(&registry);
    tokio::spawn(async move {
        let mut receiver = registry_for_task.subscribe();
        loop {
            use tokio::sync::broadcast::error::RecvError;
            match receiver.recv().await {
                Ok(_snapshot) => {
                    update_service_counters(&registry_for_task.snapshot());
                }
                Err(RecvError::Lagged(_)) => {
                    update_service_counters(&registry_for_task.snapshot());
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

pub fn start_system_metrics_sampler(cfg: &AppConfig) {
    if !cfg.telemetry.metrics_enabled {
        tracing::debug!("prozessmetriken deaktiviert (metrics_enabled=false)");
        return;
    }

    if !(cfg!(target_os = "linux") || cfg!(target_os = "macos")) {
        tracing::info!("prozessmetriken werden auf diesem betriebssystem nicht unterstützt");
        return;
    }

    let process_cfg = ProcessMetricsConfig::from(cfg);
    if !process_cfg.enabled {
        tracing::info!("prozessmetriken deaktiviert (telemetry.system.enabled=false)");
        return;
    }

    if !SYSTEM_METRICS_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        return;
    }

    if TELEMETRY.get().is_none() {
        tracing::warn!(
            "telemetry state nicht initialisiert – prozessmetriken werden nicht erhoben"
        );
        SYSTEM_METRICS_STARTED.store(false, Ordering::Release);
        return;
    }

    let interval = process_cfg.interval;
    tokio::spawn(async move {
        let mut sampler = match ProcessMetricsSampler::new() {
            Ok(sampler) => sampler,
            Err(err) => {
                tracing::warn!(error = %err, "prozessmetriken konnten nicht initialisiert werden");
                SYSTEM_METRICS_STARTED.store(false, Ordering::Release);
                return;
            }
        };
        if let Err(err) = sampler.sample_and_record() {
            tracing::debug!(error = %err, "initiales prozessmetriken-sample fehlgeschlagen");
        }

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            if let Err(err) = sampler.sample_and_record() {
                tracing::debug!(error = %err, "prozessmetriken sample fehlgeschlagen");
            }
        }
    });
}

pub fn is_ready() -> bool {
    if let Some(state) = TELEMETRY.get() {
        if !state.ready.load(Ordering::Acquire) {
            return false;
        }
        if !state.health_enabled.load(Ordering::Acquire) {
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

pub fn history(range: Duration) -> Vec<MetricHistoryPoint> {
    TELEMETRY
        .get()
        .map(|state| state.history(range))
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct TelemetrySnapshot {
    pub ready: bool,
    pub live: bool,
    pub uptime: Duration,
    pub metrics: HashMap<String, u64>,
}

struct ProcessMetricsSampler {
    ticks_per_second: u64,
    page_size: u64,
    last_cpu: Option<ProcessCpuSample>,
    last_cpu_instant: Option<Instant>,
    last_io: Option<ProcessIoSnapshot>,
    last_io_instant: Option<Instant>,
}

impl ProcessMetricsSampler {
    fn new() -> Result<Self> {
        Ok(Self {
            ticks_per_second: query_ticks_per_second()?,
            page_size: query_page_size()?,
            last_cpu: None,
            last_cpu_instant: None,
            last_io: None,
            last_io_instant: None,
        })
    }

    fn sample_and_record(&mut self) -> Result<()> {
        if let Err(err) = self.sample_memory() {
            tracing::debug!(error = %err, "prozess speicher sample fehlgeschlagen");
        }

        let now = Instant::now();

        if let Err(err) = self.sample_cpu(now) {
            tracing::debug!(error = %err, "prozess cpu sample fehlgeschlagen");
        }

        if let Err(err) = self.sample_io(now) {
            tracing::debug!(error = %err, "prozess io sample fehlgeschlagen");
        }

        record_process_history_sample();
        Ok(())
    }

    fn sample_memory(&mut self) -> Result<()> {
        match read_process_memory(self.page_size) {
            Ok(snapshot) => {
                set_counter("process.memory.virtual_bytes", snapshot.virtual_bytes);
                set_counter("process.memory.resident_bytes", snapshot.resident_bytes);
                Ok(())
            }
            Err(err) => {
                set_counter("process.memory.virtual_bytes", 0);
                set_counter("process.memory.resident_bytes", 0);
                Err(err)
            }
        }
    }

    fn sample_cpu(&mut self, now: Instant) -> Result<()> {
        match read_process_times(self.ticks_per_second) {
            Ok(times) => {
                let usage_percent = if let (Some(previous), Some(previous_instant)) =
                    (self.last_cpu, self.last_cpu_instant)
                {
                    let elapsed = now.saturating_duration_since(previous_instant);
                    if elapsed.is_zero() {
                        None
                    } else {
                        let delta_seconds = (times.total_seconds - previous.total_seconds).max(0.0);
                        Some((delta_seconds / elapsed.as_secs_f64()) * 100.0)
                    }
                } else {
                    Some(0.0)
                };

                if let Some(percent) = usage_percent {
                    let percent = percent.max(0.0).round() as u64;
                    set_counter("process.cpu.usage_percent", percent);
                }

                self.last_cpu = Some(times);
                self.last_cpu_instant = Some(now);
                Ok(())
            }
            Err(err) => {
                set_counter("process.cpu.usage_percent", 0);
                Err(err)
            }
        }
    }

    fn sample_io(&mut self, now: Instant) -> Result<()> {
        set_counter("process.io.read_bytes_per_sec", 0);
        set_counter("process.io.write_bytes_per_sec", 0);
        set_counter("process.io.read_bytes_total", 0);
        set_counter("process.io.write_bytes_total", 0);

        match read_process_io() {
            Ok(io) => {
                set_counter("process.io.read_bytes_total", io.read_bytes);
                set_counter("process.io.write_bytes_total", io.write_bytes);

                if let (Some(previous), Some(previous_instant)) =
                    (&self.last_io, self.last_io_instant)
                {
                    let elapsed = now.saturating_duration_since(previous_instant);
                    let elapsed_ms = elapsed.as_millis();
                    if elapsed_ms > 0 {
                        let read_delta = io.read_bytes.saturating_sub(previous.read_bytes);
                        let write_delta = io.write_bytes.saturating_sub(previous.write_bytes);
                        let elapsed_ms = elapsed_ms as u64;
                        let read_rate = read_delta.saturating_mul(1000) / elapsed_ms;
                        let write_rate = write_delta.saturating_mul(1000) / elapsed_ms;
                        set_counter("process.io.read_bytes_per_sec", read_rate);
                        set_counter("process.io.write_bytes_per_sec", write_rate);
                    }
                }

                self.last_io = Some(io);
                self.last_io_instant = Some(now);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

#[derive(Clone, Copy)]
struct ProcessCpuSample {
    total_seconds: f64,
}

struct ProcessMemorySnapshot {
    virtual_bytes: u64,
    resident_bytes: u64,
}

#[derive(Clone, Copy)]
struct ProcessIoSnapshot {
    read_bytes: u64,
    write_bytes: u64,
}

fn read_process_times(ticks_per_second: u64) -> Result<ProcessCpuSample> {
    read_process_times_impl(ticks_per_second)
}

#[cfg(target_os = "linux")]
fn read_process_times_impl(ticks_per_second: u64) -> Result<ProcessCpuSample> {
    let stat = fs::read_to_string("/proc/self/stat")
        .context("/proc/self/stat konnte nicht gelesen werden")?;
    let end = stat
        .rfind(')')
        .ok_or_else(|| anyhow!("/proc/self/stat format unerwartet"))?;
    let after = stat
        .get(end + 2..)
        .ok_or_else(|| anyhow!("/proc/self/stat format unerwartet"))?;
    let fields: Vec<&str> = after.split_whitespace().collect();
    if fields.len() < 15 {
        return Err(anyhow!("/proc/self/stat liefert zu wenige felder"));
    }
    let utime = fields[11]
        .parse::<u64>()
        .with_context(|| anyhow!("utime konnte nicht geparst werden"))?;
    let stime = fields[12]
        .parse::<u64>()
        .with_context(|| anyhow!("stime konnte nicht geparst werden"))?;
    let total_ticks = utime.saturating_add(stime);
    Ok(ProcessCpuSample {
        total_seconds: total_ticks as f64 / ticks_per_second as f64,
    })
}

#[cfg(target_os = "macos")]
fn read_process_times_impl(_ticks_per_second: u64) -> Result<ProcessCpuSample> {
    unsafe {
        let mut usage: libc::rusage = mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return Err(anyhow!("getrusage fehlgeschlagen"));
        }
        let user = timeval_to_secs(usage.ru_utime);
        let system = timeval_to_secs(usage.ru_stime);
        Ok(ProcessCpuSample {
            total_seconds: user + system,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_process_times_impl(_ticks_per_second: u64) -> Result<ProcessCpuSample> {
    Err(anyhow!("prozess cpu metriken nicht unterstützt"))
}

#[cfg(target_os = "macos")]
fn timeval_to_secs(tv: libc::timeval) -> f64 {
    tv.tv_sec as f64 + tv.tv_usec as f64 / 1_000_000.0
}

#[cfg(target_os = "linux")]
fn read_process_memory(page_size: u64) -> Result<ProcessMemorySnapshot> {
    let statm = fs::read_to_string("/proc/self/statm")
        .context("/proc/self/statm konnte nicht gelesen werden")?;
    let mut fields = statm.split_whitespace();
    let total_pages = fields
        .next()
        .ok_or_else(|| anyhow!("statm enthält keine größe"))?
        .parse::<u64>()
        .with_context(|| anyhow!("statm total konnte nicht geparst werden"))?;
    let resident_pages = fields
        .next()
        .ok_or_else(|| anyhow!("statm enthält keine resident größe"))?
        .parse::<u64>()
        .with_context(|| anyhow!("statm resident konnte nicht geparst werden"))?;

    Ok(ProcessMemorySnapshot {
        virtual_bytes: total_pages.saturating_mul(page_size),
        resident_bytes: resident_pages.saturating_mul(page_size),
    })
}

#[cfg(target_os = "macos")]
fn read_process_memory(_page_size: u64) -> Result<ProcessMemorySnapshot> {
    unsafe {
        let mut info: libc::mach_task_basic_info_data_t = mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info_data_t>()
            / std::mem::size_of::<libc::natural_t>()) as u32;
        #[allow(deprecated)]
        let kr = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as *mut libc::integer_t,
            &mut count,
        );
        if kr != libc::KERN_SUCCESS {
            return Err(anyhow!("task_info fehlgeschlagen: {kr}"));
        }
        Ok(ProcessMemorySnapshot {
            virtual_bytes: info.virtual_size as u64,
            resident_bytes: info.resident_size as u64,
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_process_memory(_page_size: u64) -> Result<ProcessMemorySnapshot> {
    Err(anyhow!("prozess speichermetriken nicht unterstützt"))
}

#[cfg(target_os = "linux")]
fn read_process_io() -> Result<ProcessIoSnapshot> {
    let file = File::open("/proc/self/io").context("/proc/self/io konnte nicht geöffnet werden")?;
    let reader = BufReader::new(file);
    let mut read_bytes = None;
    let mut write_bytes = None;

    for line in reader.lines() {
        let line = line.context("Fehler beim Lesen von /proc/self/io")?;
        if let Some(value) = line.strip_prefix("read_bytes:") {
            read_bytes = Some(
                value
                    .trim()
                    .parse::<u64>()
                    .with_context(|| anyhow!("read_bytes konnte nicht geparst werden"))?,
            );
        } else if let Some(value) = line.strip_prefix("write_bytes:") {
            write_bytes = Some(
                value
                    .trim()
                    .parse::<u64>()
                    .with_context(|| anyhow!("write_bytes konnte nicht geparst werden"))?,
            );
        }
    }

    Ok(ProcessIoSnapshot {
        read_bytes: read_bytes.unwrap_or(0),
        write_bytes: write_bytes.unwrap_or(0),
    })
}

#[cfg(target_os = "macos")]
fn read_process_io() -> Result<ProcessIoSnapshot> {
    Ok(ProcessIoSnapshot {
        read_bytes: 0,
        write_bytes: 0,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_process_io() -> Result<ProcessIoSnapshot> {
    Ok(ProcessIoSnapshot {
        read_bytes: 0,
        write_bytes: 0,
    })
}

fn query_ticks_per_second() -> Result<u64> {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        return Err(anyhow!("_SC_CLK_TCK liefert ungültigen wert"));
    }
    Ok(ticks as u64)
}

fn query_page_size() -> Result<u64> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(anyhow!("_SC_PAGESIZE liefert ungültigen wert"));
    }
    Ok(page_size as u64)
}

fn update_service_counters(snapshot: &[ServiceSnapshot]) {
    const STATUSES: [ServiceStatus; 6] = [
        ServiceStatus::Starting,
        ServiceStatus::Active,
        ServiceStatus::Degraded,
        ServiceStatus::Failed,
        ServiceStatus::Standby,
        ServiceStatus::Stopped,
    ];
    const TAGS: [ServiceTag; 3] = [
        ServiceTag::Core,
        ServiceTag::Platform,
        ServiceTag::Auxiliary,
    ];

    set_counter("services.total", snapshot.len() as u64);

    for status in STATUSES {
        let count = snapshot.iter().filter(|svc| svc.status == status).count() as u64;
        set_counter(&format!("services.status.{}", status.label()), count);
    }

    let critical = snapshot
        .iter()
        .filter(|svc| svc.descriptor.critical)
        .count() as u64;
    set_counter("services.critical", critical);

    for tag in TAGS {
        let count = snapshot
            .iter()
            .filter(|svc| svc.descriptor.has_tag(tag))
            .count() as u64;
        set_counter(&format!("services.tag.{}", tag.as_str()), count);
    }
}
