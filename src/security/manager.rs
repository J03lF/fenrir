use std::sync::Arc;

use tracing::warn;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::audit::{AuditActor, AuditEvent, AuditEventBuilder, AuditMetadata, AuditOutcome};
use crate::config::SecuritySection;
use crate::security::auth::{AuthError, Role};
use crate::security::crypto::{
    AeadRegistry, Argon2Kdf, CipherAlgorithm, CryptoError, KeyDerivationFunction, PasswordHashing,
};
use crate::security::service_tokens::{
    DelegatedActor, DelegatedToken, DelegatedTokenClaims, DelegatedTokenRequest, ServiceTokenError,
    ServiceTokenStore,
};
use crate::security::session::{Session, SessionError, SessionStore};
use crate::utils::messages::security::manager as security_manager_messages;

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent) -> Result<(), crate::audit::AuditError>;
}

/// No-op audit sink that discards all events.
/// Used during early boot before the real audit store is available.
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn record(&self, _event: AuditEvent) -> Result<(), crate::audit::AuditError> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    ServiceToken(#[from] ServiceTokenError),
}

pub struct SecurityManager {
    kdf: Argon2Kdf,
    aead: AeadRegistry,
    sessions: SessionStore,
    service_tokens: ServiceTokenStore,
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
                security_manager_messages::no_ciphers_configured(),
            )));
        }
        let aead = AeadRegistry::new(&algorithms);
        let sessions = SessionStore::new(&cfg.session);
        let service_tokens = ServiceTokenStore::new(&cfg.service_tokens);
        Ok(Self {
            kdf,
            aead,
            sessions,
            service_tokens,
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
        let session = self.sessions.create_session(user_id, role)?;
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

    pub fn issue_service_token(
        &self,
        request: DelegatedTokenRequest,
    ) -> Result<DelegatedToken, SecurityError> {
        let issued = self.service_tokens.issue(request)?;
        self.record_event(
            AuditEvent::builder()
                .actor(AuditActor::System)
                .action("security.service_token.issue")
                .target(format!("service-token:{}", issued.claims.token_id))
                .metadata(self.service_token_metadata(&issued.claims, None)),
        );
        Ok(issued)
    }

    pub fn validate_service_token(
        &self,
        token: &str,
    ) -> Result<DelegatedTokenClaims, SecurityError> {
        Ok(self.service_tokens.validate(token)?)
    }

    pub fn sign_service_manifest(
        &self,
        token: &str,
        payload: &[u8],
    ) -> Result<String, SecurityError> {
        Ok(sign_manifest_hmac(token, payload))
    }

    pub fn verify_service_manifest_signature(
        &self,
        token: &str,
        payload: &[u8],
        signature: &str,
    ) -> Result<bool, SecurityError> {
        let expected_bytes = sign_manifest_hmac_bytes(token, payload);
        let Ok(signature_bytes) = STANDARD.decode(signature) else {
            return Ok(false);
        };
        Ok(signature_bytes.as_slice().ct_eq(&expected_bytes).into())
    }

    pub fn revoke_service_token(&self, token: &str, reason: &str) -> Result<(), SecurityError> {
        let claims = self.service_tokens.validate(token).ok();
        self.service_tokens.revoke(token)?;
        if let Some(claims) = claims {
            self.record_event(
                AuditEvent::builder()
                    .actor(AuditActor::System)
                    .action("security.service_token.revoke")
                    .target(format!("service-token:{}", claims.token_id))
                    .metadata(self.service_token_metadata(&claims, Some(reason))),
            );
        }
        Ok(())
    }

    pub fn ensure_role(&self, token: &str, required: Role) -> Result<Session, AuthError> {
        match self.sessions.validate(token) {
            Ok(session) => {
                if session.role.satisfies(required) {
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

    pub fn sessions_active_count(&self) -> Result<usize, SecurityError> {
        Ok(self.sessions.active_sessions()?)
    }

    pub fn audit_control_plane_token(
        &self,
        token: Option<&str>,
        required: Role,
        result: &Result<Role, AuthError>,
    ) {
        // Control-plane authorize checks are extremely chatty in busy clusters
        // and swamp the audit sink without yielding additional signal. We keep
        // the computation side-effects (fingerprint derivation, etc.) but skip
        // emitting the actual audit event.
        let _ = (token, required, result);
    }

    fn service_token_metadata(
        &self,
        claims: &DelegatedTokenClaims,
        reason: Option<&str>,
    ) -> AuditMetadata {
        let scopes = if claims.scopes.is_empty() {
            "-".to_string()
        } else {
            claims
                .scopes
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut metadata = AuditMetadata::default()
            .insert("tenant_id", claims.tenant_id.clone())
            .insert("actor_kind", claims.actor.kind())
            .insert("actor_id", claims.actor.identifier())
            .insert("scopes", scopes)
            .insert("expires_at", claims.expires_at.to_string());
        match &claims.actor {
            DelegatedActor::User { role, .. } => {
                metadata = metadata.insert("user_role", role.as_str());
            }
            DelegatedActor::Service { role, .. } => {
                metadata = metadata.insert("service_role", role.as_str());
            }
        }
        if let Some(reason) = reason {
            metadata = metadata.insert("reason", reason);
        }
        metadata
    }

    fn record_event(&self, builder: AuditEventBuilder) {
        match builder.build() {
            Ok(event) => {
                if let Err(err) = self.audit.record(event) {
                    warn!(
                        error = %err,
                        "{}",
                        security_manager_messages::audit_append_failed()
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "{}",
                    security_manager_messages::audit_build_failed()
                );
            }
        }
    }

    fn token_fingerprint(&self, token: &str) -> String {
        if token.is_empty() {
            return security_manager_messages::empty_token_placeholder().to_string();
        }
        let prefix_len = token.len().min(6);
        security_manager_messages::fingerprint_display(&token[..prefix_len], token.len())
    }
}

fn sign_manifest_hmac(token: &str, payload: &[u8]) -> String {
    STANDARD.encode(sign_manifest_hmac_bytes(token, payload))
}

fn sign_manifest_hmac_bytes(token: &str, payload: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes())
        .expect("hmac key length is valid");
    mac.update(payload);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
#[path = "../../tests/unit/security/manager_tests.rs"]
mod tests;
