use std::sync::Arc;
use std::time::Instant;

use crate::security::identity::{
    IdentityClaims, IdentityError, IdentityProvider, IdentityUserProfile, IdentityUserRecord,
    IssueTokenRequest, IssuedToken,
};
use crate::services::ServiceDiagnostics;

const IDENTITY_SERVICE_ID: &str = "identity-service";

pub struct InstrumentedIdentityProvider {
    inner: Arc<dyn IdentityProvider>,
    diagnostics: Arc<ServiceDiagnostics>,
}

impl InstrumentedIdentityProvider {
    pub fn new(inner: Arc<dyn IdentityProvider>, diagnostics: Arc<ServiceDiagnostics>) -> Self {
        Self { inner, diagnostics }
    }

    fn record_result<T>(
        &self,
        result: Result<T, IdentityError>,
        started_at: Instant,
    ) -> Result<T, IdentityError> {
        let success = result.is_ok();
        self.record_metrics(started_at, success);
        result
    }

    fn record_metrics(&self, started_at: Instant, success: bool) {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe(IDENTITY_SERVICE_ID, latency_ms, success);
        self.diagnostics.record_heartbeat(IDENTITY_SERVICE_ID);
    }
}

impl IdentityProvider for InstrumentedIdentityProvider {
    fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        let started_at = Instant::now();
        let result = self.inner.issue_token(request);
        self.record_result(result, started_at)
    }

    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        let started_at = Instant::now();
        let result = self.inner.list_users();
        self.record_result(result, started_at)
    }

    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        let started_at = Instant::now();
        let result = self.inner.verify(token);
        self.record_result(result, started_at)
    }

    fn authenticate_user(
        &self,
        user_id: &str,
        password: &str,
    ) -> Result<IdentityUserProfile, IdentityError> {
        let started_at = Instant::now();
        let result = self.inner.authenticate_user(user_id, password);
        self.record_result(result, started_at)
    }
}
