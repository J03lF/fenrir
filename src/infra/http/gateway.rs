use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use reqwest::Client;
use tokio::sync::Mutex;

const DEFAULT_RATE_LIMIT_PER_WINDOW: u32 = 120;
const DEFAULT_WINDOW: Duration = Duration::from_secs(1);

pub static HTTP_GATEWAY_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .pool_max_idle_per_host(8)
        .timeout(Duration::from_secs(30))
        .build()
        .expect("gateway http client")
});

pub struct GatewayRateLimiter {
    inner: Mutex<HashMap<String, RateEntry>>,
    window: Duration,
    limit: u32,
}

struct RateEntry {
    window_start: Instant,
    count: u32,
}

impl GatewayRateLimiter {
    pub fn new(limit: u32, window: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
            window,
            limit,
        })
    }

    pub async fn try_acquire_with_limit(&self, key: &str, limit_override: Option<u32>) -> bool {
        let limit = limit_override.unwrap_or(self.limit);
        if limit == 0 {
            return true;
        }
        let mut guard = self.inner.lock().await;
        let entry = guard.entry(key.to_string()).or_insert_with(|| RateEntry {
            window_start: Instant::now(),
            count: 0,
        });
        if entry.window_start.elapsed() >= self.window {
            entry.window_start = Instant::now();
            entry.count = 0;
        }
        if entry.count >= limit {
            return false;
        }
        entry.count += 1;
        true
    }
}

pub static SERVICE_RATE_LIMITER: Lazy<Arc<GatewayRateLimiter>> =
    Lazy::new(|| GatewayRateLimiter::new(DEFAULT_RATE_LIMIT_PER_WINDOW, DEFAULT_WINDOW));
