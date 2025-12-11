use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::domain::module::{ModuleId, ModuleRuntimeError};
use crate::security::service::ServiceScope;
use crate::security::service_tokens::DelegatedToken;
use crate::services::module::{ModuleService, ModuleTokenLease};
use crate::services::ServiceDiagnostics;

const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(30);
const RATE_LIMIT_MAX_REQUESTS: u32 = 60;

struct RateBucket {
    start: Instant,
    count: u32,
}

impl RateBucket {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            count: 0,
        }
    }

    fn reset(&mut self) {
        self.start = Instant::now();
        self.count = 0;
    }
}

#[derive(Debug)]
pub enum TokenExchangeError {
    RateLimited,
    Module(ModuleRuntimeError),
}

impl From<ModuleRuntimeError> for TokenExchangeError {
    fn from(err: ModuleRuntimeError) -> Self {
        Self::Module(err)
    }
}

pub struct TokenExchangeService {
    module_service: Arc<ModuleService>,
    windows: Mutex<HashMap<String, RateBucket>>,
    diagnostics: Arc<ServiceDiagnostics>,
}

impl TokenExchangeService {
    pub fn new(module_service: Arc<ModuleService>, diagnostics: Arc<ServiceDiagnostics>) -> Self {
        Self {
            module_service,
            windows: Mutex::new(HashMap::new()),
            diagnostics,
        }
    }

    pub async fn issue_default_token(
        &self,
        module_id: &ModuleId,
    ) -> Result<DelegatedToken, TokenExchangeError> {
        self.check_rate_limit(module_id)?;
        let started_at = Instant::now();
        let token = match self.module_service.issue_default_service_token(module_id) {
            Ok(token) => token,
            Err(err) => {
                self.record_probe(started_at, false);
                return Err(TokenExchangeError::from(err));
            }
        };
        self.module_service
            .record_service_token(module_id, &token)
            .await;
        self.record_probe(started_at, true);
        Ok(token)
    }

    pub async fn issue_scoped_token(
        &self,
        module_id: &ModuleId,
        scopes: Vec<ServiceScope>,
    ) -> Result<DelegatedToken, TokenExchangeError> {
        self.check_rate_limit(module_id)?;
        let started_at = Instant::now();
        let token = match self
            .module_service
            .issue_scoped_service_token(module_id, scopes)
        {
            Ok(token) => token,
            Err(err) => {
                self.record_probe(started_at, false);
                return Err(TokenExchangeError::from(err));
            }
        };
        self.module_service
            .record_service_token(module_id, &token)
            .await;
        self.record_probe(started_at, true);
        Ok(token)
    }

    pub async fn force_refresh(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleTokenLease, ModuleRuntimeError> {
        let started_at = Instant::now();
        let lease = match self.module_service.refresh_service_token(module_id).await {
            Ok(lease) => {
                self.record_probe(started_at, true);
                lease
            }
            Err(err) => {
                self.record_probe(started_at, false);
                return Err(err);
            }
        };
        if let Ok(mut guard) = self.windows.lock() {
            if let Some(bucket) = guard.get_mut(&module_id.to_string()) {
                bucket.reset();
            }
        }
        Ok(lease)
    }

    fn check_rate_limit(&self, module_id: &ModuleId) -> Result<(), TokenExchangeError> {
        let mut guard = self
            .windows
            .lock()
            .map_err(|_| TokenExchangeError::RateLimited)?;
        let entry = guard
            .entry(module_id.to_string())
            .or_insert_with(RateBucket::new);
        if entry.start.elapsed() > RATE_LIMIT_WINDOW {
            entry.reset();
        }
        if entry.count >= RATE_LIMIT_MAX_REQUESTS {
            return Err(TokenExchangeError::RateLimited);
        }
        entry.count += 1;
        Ok(())
    }

    fn record_probe(&self, started_at: Instant, success: bool) {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe("token-exchange", latency_ms, success);
    }
}
