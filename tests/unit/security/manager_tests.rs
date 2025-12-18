use super::SecurityError;
use super::*;
use crate::audit::{AuditError, AuditEvent};
use crate::config::{
    HttpSecuritySection, IdentitySection, JwtConfig, KdfConfig, SecuritySection,
    ServiceTokenSection, SessionSection,
};
use crate::security::auth::Role;
use crate::security::service::ServiceRole;
use std::sync::Mutex;

#[derive(Default)]
struct RecordingAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl AuditSink for RecordingAuditSink {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.events.lock().unwrap().push(event);
        Ok(())
    }
}

fn security_config() -> SecuritySection {
    SecuritySection {
        kdf: KdfConfig {
            algorithm: "argon2id".to_string(),
            version: 1,
            memory_mib: 8,
            iterations: 2,
            parallelism: 1,
            salt_length: 16,
            output_length: 32,
        },
        jwt: JwtConfig {
            issuer: "fenrir".to_string(),
            audience: "aud".to_string(),
            exp_seconds: 3600,
        },
        allowed_ciphers: vec!["AES-GCM".to_string()],
        http: HttpSecuritySection {
            control_tokens: Vec::new(),
        },
        session: SessionSection {
            lifetime_seconds: 2,
            idle_timeout_seconds: 1,
            cleanup_interval_seconds: 1,
        },
        service_tokens: ServiceTokenSection {
            lifetime_seconds: 2,
            idle_timeout_seconds: 1,
            cleanup_interval_seconds: 1,
        },
        identity: IdentitySection::default(),
        password_policy: Default::default(),
    }
}

#[test]
fn sessions_and_rbac_flow() {
    let audit = Arc::new(RecordingAuditSink::default());
    let manager =
        SecurityManager::new(&security_config(), audit.clone()).expect("security manager");

    let session = manager
        .issue_session("alice", Role::Operator)
        .expect("session creation");
    assert!(manager.ensure_role(&session.id, Role::Viewer).is_ok());
    assert!(matches!(
        manager.ensure_role(&session.id, Role::Admin),
        Err(AuthError::Forbidden)
    ));

    manager.revoke_session(&session.id, "test").expect("revoke");
    assert!(matches!(
        manager.validate_session(&session.id),
        Err(SecurityError::Session(
            SessionError::NotFound | SessionError::Expired | SessionError::IdleTimeout
        ))
    ));

    let events = audit.events.lock().unwrap();
    assert!(!events.is_empty());
}

#[test]
fn service_tokens_issue_and_revoke() {
    let audit = Arc::new(RecordingAuditSink::default());
    let manager = SecurityManager::new(&security_config(), audit).expect("security manager");
    let issued = manager
        .issue_service_token(DelegatedTokenRequest::new(
            DelegatedActor::Service {
                service_id: "module:test".to_string(),
                role: ServiceRole::Write,
            },
            "tenant-1",
        ))
        .expect("token issued");
    assert_eq!(issued.claims.tenant_id, "tenant-1");
    let validated = manager
        .validate_service_token(&issued.token)
        .expect("token is valid");
    assert_eq!(validated.actor.kind(), "service");
    manager
        .revoke_service_token(&issued.token, "cleanup")
        .expect("token revoked");
    assert!(matches!(
        manager.validate_service_token(&issued.token),
        Err(SecurityError::ServiceToken(ServiceTokenError::NotFound))
            | Err(SecurityError::ServiceToken(ServiceTokenError::Expired))
            | Err(SecurityError::ServiceToken(ServiceTokenError::IdleTimeout))
    ));
}

#[test]
fn hash_and_verify_passwords() {
    let audit = Arc::new(RecordingAuditSink::default());
    let manager = SecurityManager::new(&security_config(), audit).expect("security manager");
    let hash = manager.hash_password(b"swordfish").expect("hash password");
    assert!(manager
        .verify_password(b"swordfish", &hash)
        .expect("verify success"));
    assert!(!manager
        .verify_password(b"wrong", &hash)
        .expect("verify runs"));
}
