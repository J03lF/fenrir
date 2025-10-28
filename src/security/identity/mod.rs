mod external;
mod store;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use tracing::warn;
use uuid::Uuid;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata};
use crate::config::{AppConfig, IdentityProviderKind};
use crate::security::auth::{AuthError, Role};
use crate::security::manager::AuditSink;
use reqwest::Url;

use external::ExternalIdentityProvider;
pub use store::{IdentityKeyMaterial, IdentityStore, IdentityUserRecord};

pub trait IdentityProvider: Send + Sync {
    fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError>;
    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError>;
    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError>;
}

#[derive(thiserror::Error, Debug)]
pub enum IdentityError {
    #[error("identity store io error: {0}")]
    Io(#[source] std::io::Error),
    #[error("identity store serialization error: {0}")]
    Serde(#[source] serde_json::Error),
    #[error("identity state poisoned")]
    StatePoisoned,
    #[error("identity data invalid: {0}")]
    Invalid(String),
    #[error("identity authorization failed: {0}")]
    Unauthorized(String),
}

impl From<IdentityError> for AuthError {
    fn from(err: IdentityError) -> Self {
        match err {
            IdentityError::Unauthorized(_) => AuthError::Unauthorized,
            _ => AuthError::Forbidden,
        }
    }
}

pub struct IdentityAuthority {
    environment: String,
    instance_id: String,
    issuer: String,
    audience: String,
    app_version: String,
    exp_seconds: u64,
    store: IdentityStore,
    audit: Arc<dyn AuditSink>,
}

#[derive(Clone)]
pub struct IssueTokenRequest {
    pub actor: AuditActor,
    pub user_id: String,
    pub display_name: Option<String>,
    pub role: Role,
}

#[derive(Clone, Debug)]
pub struct IssuedToken {
    pub token: String,
    pub expires_at: OffsetDateTime,
    pub user_id: String,
    pub key_id: String,
    pub fingerprint: String,
    pub token_id: String,
}

#[derive(Clone, Debug)]
pub struct IdentityClaims {
    pub user_id: String,
    pub role: Role,
    pub environment: String,
    pub instance_id: String,
    pub app_version: String,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub key_id: String,
    pub token_id: String,
}

impl IdentityAuthority {
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
        let store = IdentityStore::load_or_initialize(
            store_path,
            identity_cfg.environment.clone(),
            identity_cfg.instance_id.clone(),
            cfg.app.version.clone(),
        )?;
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
}

impl IdentityAuthority {
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
                    role: role.clone(),
                    created_at: now,
                    last_issued_at: None,
                    token_count: 0,
                    last_token_fingerprint: None,
                });
            record.role = role.clone();
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
            Ok::<_, IdentityError>(IssuedToken {
                token,
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
                "unsupported signing algorithm".into(),
            ));
        }
        let state_snapshot = self.store.read(|state| state.clone())?;
        if raw_header.kid != state_snapshot.current_key.key_id {
            return Err(IdentityError::Unauthorized("unknown signing key".into()));
        }

        let signing_input = format!(
            "{}.{}",
            raw_header.encoded_header, raw_claims.encoded_claims
        );
        state_snapshot
            .current_key
            .verifying_key()?
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| IdentityError::Unauthorized("signature verification failed".into()))?;

        let claims: Claims = serde_json::from_slice(&raw_claims.decoded)?;

        if claims.iss != self.issuer {
            return Err(IdentityError::Unauthorized("issuer mismatch".into()));
        }
        if claims.aud != self.audience {
            return Err(IdentityError::Unauthorized("audience mismatch".into()));
        }
        if claims.env != self.environment {
            return Err(IdentityError::Unauthorized("environment mismatch".into()));
        }
        if claims.exp <= OffsetDateTime::now_utc().unix_timestamp() {
            return Err(IdentityError::Unauthorized("token expired".into()));
        }

        let role = match claims.role.as_str() {
            "admin" => Role::Admin,
            "operator" => Role::Operator,
            "viewer" => Role::Viewer,
            other => {
                return Err(IdentityError::Unauthorized(format!(
                    "unknown role claim '{other}'"
                )))
            }
        };

        Ok(IdentityClaims {
            user_id: claims.sub,
            role,
            environment: claims.env,
            instance_id: claims.inst,
            app_version: claims.ver,
            issued_at: OffsetDateTime::from_unix_timestamp(claims.iat)
                .map_err(|_| IdentityError::Invalid("invalid issued-at timestamp".into()))?,
            expires_at: OffsetDateTime::from_unix_timestamp(claims.exp)
                .map_err(|_| IdentityError::Invalid("invalid expiry timestamp".into()))?,
            key_id: raw_header.kid,
            token_id: claims.jti,
        })
    }

    pub fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        self.store
            .read(|state| state.users.values().cloned().collect())
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
                    warn!(error = %err, "failed to append identity audit event");
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to build identity audit event");
            }
        }
    }
}

pub fn build_identity_provider(
    cfg: &AppConfig,
    runtime_dir: &Path,
    audit: Arc<dyn AuditSink>,
) -> Result<Arc<dyn IdentityProvider>, IdentityError> {
    match cfg.security.identity.provider {
        IdentityProviderKind::Embedded => {
            let provider = IdentityAuthority::bootstrap(cfg, runtime_dir, Arc::clone(&audit))?;
            Ok(Arc::new(provider))
        }
        IdentityProviderKind::External => {
            let identity_cfg = &cfg.security.identity;
            let base_url = parse_url(
                identity_cfg
                    .external
                    .base_url
                    .as_deref()
                    .expect("validated base_url must be present"),
            )?;
            let jwks_url = if let Some(url) = &identity_cfg.external.jwks_url {
                parse_url(url)?
            } else {
                base_url.join("jwks.json").map_err(|err| {
                    IdentityError::Invalid(format!("failed to derive JWKS url: {err}"))
                })?
            };
            let auth_token = identity_cfg
                .resolve_external_auth_token()
                .map_err(|err| IdentityError::Invalid(format!("{err}")))?;
            let provider = ExternalIdentityProvider::new(
                base_url,
                jwks_url,
                auth_token,
                identity_cfg.environment.clone(),
                identity_cfg.instance_id.clone(),
                identity_cfg.external.audience.clone(),
                cfg.app.version.clone(),
                identity_cfg.external.jwks_refresh_seconds,
                audit,
            )?;
            Ok(Arc::new(provider))
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
}

fn issued_role(store: &IdentityStore, user_id: &str) -> Option<String> {
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

fn parse_url(value: &str) -> Result<Url, IdentityError> {
    Url::parse(value)
        .map_err(|err| IdentityError::Invalid(format!("invalid identity url '{value}': {err}")))
}

fn encode_jwt(key: &IdentityKeyMaterial, claims: &RawClaims) -> Result<String, IdentityError> {
    let header = RawHeader {
        alg: "EdDSA".to_string(),
        typ: "JWT".to_string(),
        kid: key.key_id.clone(),
    };
    let header_json = serde_json::to_vec(&header)?;
    let claims_json = serde_json::to_vec(claims)?;
    let header_b64 = URL_SAFE_NO_PAD.encode(header_json);
    let claims_b64 = URL_SAFE_NO_PAD.encode(&claims_json);
    let signing_input = format!("{header_b64}.{claims_b64}");
    let keypair = key.keypair()?;
    let signature = keypair.sign(signing_input.as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    Ok(format!("{signing_input}.{signature_b64}"))
}

pub(super) fn fingerprint_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

pub(super) struct ParsedHeader {
    encoded_header: String,
    alg: String,
    kid: String,
}

pub(super) struct ParsedClaims {
    encoded_claims: String,
    decoded: Vec<u8>,
}

pub(super) fn parse_jwt(
    token: &str,
) -> Result<(ParsedHeader, ParsedClaims, Signature), IdentityError> {
    let mut parts = token.split('.');
    let header = parts
        .next()
        .ok_or_else(|| IdentityError::Unauthorized("invalid token: missing header".into()))?;
    let claims = parts
        .next()
        .ok_or_else(|| IdentityError::Unauthorized("invalid token: missing claims".into()))?;
    let signature = parts
        .next()
        .ok_or_else(|| IdentityError::Unauthorized("invalid token: missing signature".into()))?;
    if parts.next().is_some() {
        return Err(IdentityError::Unauthorized(
            "invalid token: too many segments".into(),
        ));
    }

    let header_bytes = URL_SAFE_NO_PAD
        .decode(header)
        .map_err(|_| IdentityError::Unauthorized("invalid token header encoding".into()))?;
    let header_obj: Header = serde_json::from_slice(&header_bytes)
        .map_err(|_| IdentityError::Unauthorized("invalid token header payload".into()))?;

    let claims_bytes = URL_SAFE_NO_PAD
        .decode(claims)
        .map_err(|_| IdentityError::Unauthorized("invalid token claims encoding".into()))?;

    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| IdentityError::Unauthorized("invalid signature encoding".into()))?;
    let signature = Signature::from_bytes(&signature_bytes)
        .map_err(|_| IdentityError::Unauthorized("invalid signature length".into()))?;

    Ok((
        ParsedHeader {
            encoded_header: header.to_string(),
            alg: header_obj.alg,
            kid: header_obj.kid,
        },
        ParsedClaims {
            encoded_claims: claims.to_string(),
            decoded: claims_bytes,
        },
        signature,
    ))
}

#[derive(Serialize, Deserialize)]
pub(super) struct RawHeader {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Deserialize)]
pub(super) struct Header {
    alg: String,
    #[allow(dead_code)]
    typ: String,
    kid: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct RawClaims {
    iss: String,
    aud: String,
    sub: String,
    env: String,
    inst: String,
    role: String,
    ver: String,
    iat: i64,
    exp: i64,
    jti: String,
}

#[derive(Deserialize)]
pub(super) struct Claims {
    iss: String,
    aud: String,
    sub: String,
    env: String,
    inst: String,
    role: String,
    ver: String,
    iat: i64,
    exp: i64,
    jti: String,
}
