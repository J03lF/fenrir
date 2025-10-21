use std::sync::Arc;

use tracing::warn;

use crate::audit::{AuditActor, AuditEvent, AuditEventBuilder, AuditMetadata, AuditOutcome};
use crate::config::SecuritySection;
use crate::security::auth::{AuthError, Role};
use crate::security::crypto::{
    AeadRegistry, Argon2Kdf, CipherAlgorithm, CryptoError, KeyDerivationFunction, PasswordHashing,
};
use crate::security::session::{Session, SessionError, SessionStore};

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent) -> Result<(), crate::audit::AuditError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

pub struct SecurityManager {
    kdf: Argon2Kdf,
    aead: AeadRegistry,
    sessions: SessionStore,
    audit: Arc<dyn AuditSink>,
}

impl SecurityManager {
    pub fn new(cfg: &SecuritySection, audit: Arc<dyn AuditSink>) -> Result<Self, SecurityError> {
        let kdf = Argon2Kdf::from_config(&cfg.kdf)?;
        let mut algorithms = Vec::with_capacity(cfg.allowed_ciphers.len());
        for value in &cfg.allowed_ciphers {
            algorithms.push(CipherAlgorithm::try_from(value.as_str())?);
        }
        if algorithms.is_empty() {
            return Err(SecurityError::Crypto(CryptoError::UnsupportedAlgorithm(
                "no ciphers configured".into(),
            )));
        }
        let aead = AeadRegistry::new(&algorithms);
        let sessions = SessionStore::new(&cfg.session);
        Ok(Self {
            kdf,
            aead,
            sessions,
            audit,
        })
    }

    pub fn hash_password(&self, password: &[u8]) -> Result<String, SecurityError> {
        Ok(self.kdf.hash_password(password)?)
    }

    pub fn verify_password(&self, password: &[u8], hash: &str) -> Result<bool, SecurityError> {
        Ok(self.kdf.verify_password(password, hash)?)
    }

    pub fn derive_key(&self, password: &[u8], salt: &[u8]) -> Result<Vec<u8>, SecurityError> {
        Ok(self.kdf.derive_key(password, salt)?)
    }

    pub fn encrypt(
        &self,
        algorithm: CipherAlgorithm,
        key: &[u8],
        nonce: &[u8],
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        Ok(self.aead.encrypt(algorithm, key, nonce, plaintext, aad)?)
    }

    pub fn decrypt(
        &self,
        algorithm: CipherAlgorithm,
        key: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        aad: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        Ok(self.aead.decrypt(algorithm, key, nonce, ciphertext, aad)?)
    }

    pub fn generate_nonce(&self, algorithm: CipherAlgorithm) -> Result<Vec<u8>, SecurityError> {
        Ok(self.aead.generate_nonce(algorithm)?)
    }

    pub fn issue_session(&self, user_id: &str, role: Role) -> Result<Session, SecurityError> {
        let session = self.sessions.create_session(user_id, role.clone())?;
        self.record_event(
            AuditEvent::builder()
                .actor(AuditActor::User {
                    user_id: user_id.to_string(),
                    role: role.as_str().to_string(),
                })
                .action("security.session.issue")
                .target(format!("session:{}", self.token_fingerprint(&session.id)))
                .metadata(
                    AuditMetadata::default()
                        .insert("role", role.as_str())
                        .insert("expires_at", session.expires_at.to_string()),
                ),
        );
        Ok(session)
    }

    pub fn validate_session(&self, token: &str) -> Result<Session, SecurityError> {
        let session = self.sessions.validate(token)?;
        Ok(session)
    }

    pub fn revoke_session(&self, token: &str, reason: &str) -> Result<(), SecurityError> {
        let session = self.sessions.validate(token).ok();
        match self.sessions.revoke(token) {
            Ok(()) | Err(SessionError::NotFound) => {}
            Err(err) => return Err(err.into()),
        }
        if let Some(session) = session {
            self.record_event(
                AuditEvent::builder()
                    .actor(AuditActor::System)
                    .action("security.session.revoke")
                    .target(format!("session:{}", self.token_fingerprint(&session.id)))
                    .metadata(
                        AuditMetadata::default()
                            .insert("user_id", session.user_id)
                            .insert("role", session.role.as_str())
                            .insert("reason", reason),
                    ),
            );
        }
        Ok(())
    }

    pub fn ensure_role(&self, token: &str, required: Role) -> Result<Session, AuthError> {
        match self.sessions.validate(token) {
            Ok(session) => {
                if session.role.satisfies(required.clone()) {
                    self.record_event(
                        AuditEvent::builder()
                            .actor(AuditActor::User {
                                user_id: session.user_id.clone(),
                                role: session.role.as_str().to_string(),
                            })
                            .action("security.rbac.allow")
                            .target(format!("session:{}", self.token_fingerprint(&session.id)))
                            .metadata(
                                AuditMetadata::default()
                                    .insert("required_role", required.as_str())
                                    .insert("granted_role", session.role.as_str()),
                            ),
                    );
                    Ok(session)
                } else {
                    self.record_event(
                        AuditEvent::builder()
                            .actor(AuditActor::User {
                                user_id: session.user_id.clone(),
                                role: session.role.as_str().to_string(),
                            })
                            .action("security.rbac.denied")
                            .outcome(AuditOutcome::Denied)
                            .target(format!("session:{}", self.token_fingerprint(&session.id)))
                            .metadata(
                                AuditMetadata::default()
                                    .insert("required_role", required.as_str())
                                    .insert("granted_role", session.role.as_str()),
                            ),
                    );
                    Err(AuthError::Forbidden)
                }
            }
            Err(err) => {
                self.record_event(
                    AuditEvent::builder()
                        .actor(AuditActor::System)
                        .action("security.rbac.denied")
                        .outcome(AuditOutcome::Denied)
                        .target("session:invalid")
                        .metadata(
                            AuditMetadata::default()
                                .insert("reason", format!("{}", err))
                                .insert("required_role", required.as_str()),
                        ),
                );
                match err {
                    SessionError::NotFound | SessionError::Expired | SessionError::IdleTimeout => {
                        Err(AuthError::Unauthorized)
                    }
                    SessionError::Store => Err(AuthError::Unauthorized),
                }
            }
        }
    }

    pub fn audit_control_plane_token(
        &self,
        token: Option<&str>,
        required: Role,
        result: &Result<Role, AuthError>,
    ) {
        let (outcome, granted_role) = match result {
            Ok(role) if role.satisfies(required.clone()) => (AuditOutcome::Success, Some(role)),
            Ok(role) => (AuditOutcome::Denied, Some(role)),
            Err(_) => (AuditOutcome::Denied, None),
        };
        let fingerprint = token
            .map(|value| self.token_fingerprint(value))
            .unwrap_or_else(|| "<none>".to_string());
        let mut metadata = AuditMetadata::default().insert("required_role", required.as_str());
        if let Some(role) = granted_role {
            metadata = metadata.insert("granted_role", role.as_str());
        }
        self.record_event(
            AuditEvent::builder()
                .actor(AuditActor::System)
                .action("security.control_plane.authorize")
                .outcome(outcome)
                .target(format!("token:{}", fingerprint))
                .metadata(metadata),
        );
    }

    fn record_event(&self, builder: AuditEventBuilder) {
        match builder.build() {
            Ok(event) => {
                if let Err(err) = self.audit.record(event) {
                    warn!(error = %err, "failed to append security audit event");
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to build security audit event");
            }
        }
    }

    fn token_fingerprint(&self, token: &str) -> String {
        if token.is_empty() {
            return "<empty>".to_string();
        }
        let prefix_len = token.len().min(6);
        format!("{}…({} bytes)", &token[..prefix_len], token.len())
    }
}

#[cfg(test)]
mod tests {
    use super::SecurityError;
    use super::*;
    use crate::audit::{AuditError, AuditEvent};
    use crate::config::{
        HttpSecuritySection, JwtConfig, KdfConfig, SecuritySection, SessionSection,
    };
    use crate::security::auth::Role;
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
}
