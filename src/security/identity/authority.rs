use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use argon2::password_hash::PasswordHash;
use argon2::{Argon2, PasswordVerifier};
use ed25519_dalek::Verifier;
use time::{Duration, OffsetDateTime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata};
use crate::config::{AppConfig, IdentityStorageKind};
use crate::security::auth::Role;
use crate::security::manager::AuditSink;
use crate::services::db_shell::DbShellService;
use crate::utils::messages::security::identity as identity_messages;

use super::any_store::AnyIdentityStore;
use super::db_store::DbIdentityStore;
use super::errors::IdentityError;
use super::jwt::{encode_jwt, fingerprint_token, parse_jwt, Claims, RawClaims};
use super::provider::IdentityProvider;
use super::store::{IdentityKeyMaterial, IdentityStore, IdentityTokenRecord, IdentityUserRecord};
use super::types::{IdentityClaims, IdentityUserProfile, IssueTokenRequest, IssuedToken};

pub struct IdentityAuthority {
    environment: String,
    instance_id: String,
    issuer: String,
    audience: String,
    app_version: String,
    exp_seconds: u64,
    store: AnyIdentityStore,
    audit: Arc<dyn AuditSink>,
}

const MAX_TOKENS_PER_USER: usize = 20;

impl IdentityAuthority {
    /// Bootstrap identity authority with file-based storage (default)
    pub fn bootstrap(
        cfg: &AppConfig,
        runtime_dir: &Path,
        audit: Arc<dyn AuditSink>,
    ) -> Result<Self, IdentityError> {
        let identity_cfg = &cfg.security.identity;
        let issuer = format!(
            "urn:fenrir:{}:{}",
            identity_cfg.environment, identity_cfg.instance_id
        );
        let audience = identity_cfg.audience().to_string();
        let exp_seconds = cfg.security.jwt.exp_seconds;
        let store_path = resolve_store_path(runtime_dir, identity_cfg.store_path());
        let file_store = IdentityStore::load_or_initialize(
            store_path,
            identity_cfg.environment.clone(),
            identity_cfg.instance_id.clone(),
            cfg.app.version.clone(),
        )?;
        info!("identity store: using file backend");
        Ok(Self {
            environment: identity_cfg.environment.clone(),
            instance_id: identity_cfg.instance_id.clone(),
            issuer,
            audience,
            app_version: cfg.app.version.clone(),
            exp_seconds,
            store: AnyIdentityStore::File(file_store),
            audit,
        })
    }

    /// Bootstrap identity authority with storage backend based on config
    pub fn bootstrap_with_storage(
        cfg: &AppConfig,
        runtime_dir: &Path,
        db_shell: Option<Arc<DbShellService>>,
        audit: Arc<dyn AuditSink>,
    ) -> Result<Self, IdentityError> {
        let identity_cfg = &cfg.security.identity;
        let storage_kind = identity_cfg.embedded.storage;

        let issuer = format!(
            "urn:fenrir:{}:{}",
            identity_cfg.environment, identity_cfg.instance_id
        );
        let audience = identity_cfg.audience().to_string();
        let exp_seconds = cfg.security.jwt.exp_seconds;

        let store = match storage_kind {
            IdentityStorageKind::File => {
                let store_path = resolve_store_path(runtime_dir, identity_cfg.store_path());
                let file_store = IdentityStore::load_or_initialize(
                    store_path,
                    identity_cfg.environment.clone(),
                    identity_cfg.instance_id.clone(),
                    cfg.app.version.clone(),
                )?;
                info!("identity store: using file backend");
                AnyIdentityStore::File(file_store)
            }
            IdentityStorageKind::Db => {
                let db_shell = db_shell.ok_or_else(|| {
                    IdentityError::Invalid(
                        "DB storage requested but DbShellService not available".into(),
                    )
                })?;
                let db_store = DbIdentityStore::new(
                    db_shell,
                    identity_cfg.environment.clone(),
                    identity_cfg.instance_id.clone(),
                    cfg.app.version.clone(),
                )?
                // Set the directory for pending password file (from setup script)
                // runtime_dir is already the identity directory
                .with_pending_password_dir(runtime_dir.to_path_buf());
                // Initialize DB store (create key if needed)
                db_store.initialize()?;
                info!("identity store: using database backend");
                AnyIdentityStore::Db(db_store)
            }
        };

        Ok(Self {
            environment: identity_cfg.environment.clone(),
            instance_id: identity_cfg.instance_id.clone(),
            issuer,
            audience,
            app_version: cfg.app.version.clone(),
            exp_seconds,
            store,
            audit,
        })
    }

    pub fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        let IssueTokenRequest {
            actor,
            user_id,
            display_name,
            role,
        } = request;
        let now = OffsetDateTime::now_utc();
        let ttl = i64::try_from(self.exp_seconds).unwrap_or(i64::MAX);
        let expires_at = now + Duration::seconds(ttl);
        let issued_result = self.store.write(|state| {
            let key = state.current_key.clone();
            let record = state
                .users
                .entry(user_id.clone())
                .or_insert_with(|| IdentityUserRecord {
                    user_id: user_id.clone(),
                    display_name: display_name.clone(),
                    role,
                    created_at: now,
                    last_issued_at: None,
                    token_count: 0,
                    last_token_fingerprint: None,
                    tokens: Vec::new(),
                    password_hash: None,
                    password_updated_at: None,
                    last_login_at: None,
                });
            record.role = role;
            if let Some(name) = display_name
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                record.display_name = Some(name.to_string());
            }
            record.last_issued_at = Some(now);
            record.token_count = record.token_count.saturating_add(1);

            let token_id = Uuid::new_v4().to_string();
            let claims = RawClaims {
                iss: self.issuer.clone(),
                aud: self.audience.clone(),
                sub: user_id.clone(),
                env: self.environment.clone(),
                inst: self.instance_id.clone(),
                role: role.as_str().to_string(),
                ver: self.app_version.clone(),
                iat: now.unix_timestamp(),
                exp: expires_at.unix_timestamp(),
                jti: token_id.clone(),
            };
            let token = encode_jwt(&key, &claims)?;
            let fingerprint = fingerprint_token(&token);
            record.last_token_fingerprint = Some(fingerprint.clone());
            record.tokens.insert(
                0,
                IdentityTokenRecord {
                    token_id: token_id.clone(),
                    fingerprint: fingerprint.clone(),
                    issued_at: now,
                    expires_at,
                    key_id: key.key_id.clone(),
                },
            );
            if record.tokens.len() > MAX_TOKENS_PER_USER {
                record.tokens.truncate(MAX_TOKENS_PER_USER);
            }
            Ok::<_, IdentityError>(IssuedToken {
                token,
                issued_at: now,
                expires_at,
                user_id: user_id.clone(),
                key_id: key.key_id.clone(),
                fingerprint,
                token_id,
            })
        })?;

        self.record_issue_audit(actor, &issued_result, Some(role.as_str().to_string()));
        Ok(issued_result)
    }

    pub fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        let (raw_header, raw_claims, signature) = parse_jwt(token)?;
        if raw_header.alg != "EdDSA" {
            return Err(IdentityError::Unauthorized(
                identity_messages::unsupported_signing_algorithm().into(),
            ));
        }
        let state_snapshot = self.store.read(|state| state.clone())?;
        if raw_header.kid != state_snapshot.current_key.key_id {
            return Err(IdentityError::Unauthorized(
                identity_messages::unknown_signing_key().into(),
            ));
        }

        let signing_input = format!(
            "{}.{}",
            raw_header.encoded_header, raw_claims.encoded_claims
        );
        state_snapshot
            .current_key
            .verifying_key()?
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| {
                IdentityError::Unauthorized(
                    identity_messages::signature_verification_failed().into(),
                )
            })?;

        let claims: Claims = serde_json::from_slice(&raw_claims.decoded).map_err(|_| {
            IdentityError::Unauthorized(identity_messages::invalid_token_claims_payload().into())
        })?;

        if claims.iss != self.issuer {
            return Err(IdentityError::Unauthorized(
                identity_messages::issuer_mismatch().into(),
            ));
        }
        if claims.aud != self.audience {
            return Err(IdentityError::Unauthorized(
                identity_messages::audience_mismatch().into(),
            ));
        }
        if claims.env != self.environment {
            return Err(IdentityError::Unauthorized(
                identity_messages::environment_mismatch().into(),
            ));
        }
        if claims.exp <= OffsetDateTime::now_utc().unix_timestamp() {
            return Err(IdentityError::Unauthorized(
                identity_messages::token_expired().into(),
            ));
        }

        let role = Role::from_str(claims.role.as_str()).map_err(|err| {
            IdentityError::Unauthorized(identity_messages::unknown_role_claim(err.value()))
        })?;

        Ok(IdentityClaims {
            user_id: claims.sub,
            role,
            environment: claims.env,
            instance_id: claims.inst,
            app_version: claims.ver,
            issued_at: OffsetDateTime::from_unix_timestamp(claims.iat).map_err(|_| {
                IdentityError::Invalid(identity_messages::invalid_issued_at_timestamp().into())
            })?,
            expires_at: OffsetDateTime::from_unix_timestamp(claims.exp).map_err(|_| {
                IdentityError::Invalid(identity_messages::invalid_expiry_timestamp().into())
            })?,
            key_id: raw_header.kid,
            token_id: claims.jti,
        })
    }

    pub fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        self.store.read(|state| {
            state
                .users
                .values()
                .cloned()
                .map(|mut record| {
                    record.password_hash = None;
                    record
                })
                .collect()
        })
    }

    pub fn current_key(&self) -> Result<IdentityKeyMaterial, IdentityError> {
        self.store.read(|state| state.current_key.clone())
    }

    fn record_issue_audit(
        &self,
        actor: AuditActor,
        issued: &IssuedToken,
        role_label: Option<String>,
    ) {
        let metadata = AuditMetadata::default()
            .insert("environment", self.environment.clone())
            .insert("instance", self.instance_id.clone())
            .insert("key_id", issued.key_id.clone())
            .insert("fingerprint", issued.fingerprint.clone())
            .insert("issued_at", issued.issued_at.to_string())
            .insert("expires_at", issued.expires_at.to_string())
            .insert("token_id", issued.token_id.clone());
        let metadata = metadata.insert(
            "role",
            role_label
                .or_else(|| issued_role(&self.store, &issued.user_id))
                .unwrap_or_else(|| "unknown".to_string()),
        );

        match AuditEvent::builder()
            .actor(actor)
            .action("identity.token.issue")
            .target(format!(
                "identity://{}/{}",
                self.environment, issued.user_id
            ))
            .metadata(metadata)
            .build()
        {
            Ok(event) => {
                if let Err(err) = self.audit.record(event) {
                    warn!(
                        error = %err,
                        "{}",
                        identity_messages::audit_append_failed()
                    );
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "{}",
                    identity_messages::audit_build_failed()
                );
            }
        }
    }
}

impl IdentityProvider for IdentityAuthority {
    fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        IdentityAuthority::issue_token(self, request)
    }

    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        IdentityAuthority::list_users(self)
    }

    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        IdentityAuthority::verify(self, token)
    }

    fn authenticate_user(
        &self,
        user_id: &str,
        password: &str,
    ) -> Result<IdentityUserProfile, IdentityError> {
        // Get user from store - if not found, treat as first-time setup
        let user = match self.store.get_user(user_id)? {
            Some(u) => u,
            None => {
                // User doesn't exist yet - trigger first-time setup
                return Err(IdentityError::PasswordNotSet {
                    user_id: user_id.to_string(),
                });
            }
        };

        // Check if password is set
        let hash = user
            .password_hash
            .as_ref()
            .ok_or_else(|| IdentityError::PasswordNotSet {
                user_id: user_id.to_string(),
            })?;

        // Verify password with Argon2
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|_| IdentityError::Invalid("invalid password hash format".into()))?;

        let argon2 = Argon2::default();
        if argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_err()
        {
            return Err(IdentityError::Unauthorized(
                identity_messages::invalid_credentials().into(),
            ));
        }

        // Record successful login
        let _ = self.store.record_login(user_id);

        Ok(IdentityUserProfile {
            user_id: user.user_id,
            display_name: user.display_name,
            role: user.role,
            password_updated_at: user.password_updated_at,
            last_login_at: user.last_login_at,
        })
    }

    fn set_user_password(
        &self,
        user_id: &str,
        password_hash: &str,
        role: Role,
    ) -> Result<(), IdentityError> {
        self.store
            .set_password(user_id, password_hash.to_string(), role)
    }

    fn is_password_set(&self, user_id: &str) -> Result<bool, IdentityError> {
        self.store.is_password_set(user_id)
    }

    fn take_pending_password(&self) -> Option<String> {
        self.store.take_pending_password()
    }
}

fn issued_role(store: &AnyIdentityStore, user_id: &str) -> Option<String> {
    store
        .read(|state| {
            state
                .users
                .get(user_id)
                .map(|user| user.role.as_str().to_string())
        })
        .ok()
        .flatten()
}

fn resolve_store_path(runtime_dir: &Path, configured: PathBuf) -> PathBuf {
    if configured.is_absolute() {
        configured
    } else {
        runtime_dir.join(configured)
    }
}
