use std::sync::Arc;

use tracing::warn;

use crate::infra::telemetry;
use crate::security::auth::{AuthError, Role};
use crate::security::manager::{SecurityError, SecurityManager};
use crate::security::session::Session;
use crate::utils::messages::services::security::logs as security_logs;

pub struct SessionService {
    manager: Arc<SecurityManager>,
}

impl SessionService {
    pub fn new(manager: Arc<SecurityManager>) -> Self {
        let service = Self { manager };
        service.refresh_active_metric();
        service
    }

    pub fn issue_session(&self, user_id: &str, role: Role) -> Result<Session, SecurityError> {
        let session = self.manager.issue_session(user_id, role)?;
        telemetry::record_counter("security.sessions.issued_total", 1);
        self.refresh_active_metric();
        Ok(session)
    }

    pub fn validate_session(&self, token: &str) -> Result<Session, SecurityError> {
        match self.manager.validate_session(token) {
            Ok(session) => {
                telemetry::record_counter("security.sessions.validated_total", 1);
                Ok(session)
            }
            Err(err) => {
                telemetry::record_counter("security.sessions.validation_failed_total", 1);
                Err(err)
            }
        }
    }

    pub fn revoke_session(&self, token: &str, reason: &str) -> Result<(), SecurityError> {
        self.manager.revoke_session(token, reason)?;
        telemetry::record_counter("security.sessions.revoked_total", 1);
        self.refresh_active_metric();
        Ok(())
    }

    pub fn ensure_role(&self, token: &str, required: Role) -> Result<Session, AuthError> {
        self.manager.ensure_role(token, required)
    }

    pub fn active_sessions(&self) -> Option<usize> {
        match self.manager.sessions_active_count() {
            Ok(count) => Some(count),
            Err(err) => {
                warn!(error = %err, "{}", security_logs::ACTIVE_SESSIONS_FAILED);
                None
            }
        }
    }

    fn refresh_active_metric(&self) {
        match self.manager.sessions_active_count() {
            Ok(count) => telemetry::set_counter("security.sessions.active", count as u64),
            Err(err) => warn!(
                error = %err,
                "{}",
                security_logs::ACTIVE_METRIC_FAILED
            ),
        }
    }
}
