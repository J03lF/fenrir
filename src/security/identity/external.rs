use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{PublicKey, Verifier};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Certificate, Identity, Url};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::{debug, warn};

use super::jwt::{fingerprint_token, parse_jwt, Claims};
use super::{
    IdentityClaims, IdentityError, IdentityProvider, IdentityTokenRecord, IdentityUserProfile,
    IdentityUserRecord, IssueTokenRequest, IssuedToken,
};
use crate::audit::{AuditActor, AuditEvent, AuditMetadata};
use crate::security::auth::Role;
use crate::security::manager::AuditSink;
use crate::utils::messages::security::identity as identity_messages;

const TOKENS_ISSUE_PATH: &str = "tokens/issue";
const USERS_LIST_PATH: &str = "users";
const LOGIN_PATH: &str = "sessions/login";

pub struct ExternalIdentityProvider {
    base_url: Url,
    jwks_url: Url,
    auth_token: Option<String>,
    environment: String,
    instance_id: String,
    audience: String,
    app_version: String,
    audit: Arc<dyn AuditSink>,
    jwks_cache: RwLock<Option<JwksCache>>,
    jwks_ttl: Duration,
    tls: ExternalIdentityTlsOptions,
}

struct JwksCache {
    keys: HashMap<String, PublicKey>,
    expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct ExternalIdentityTlsOptions {
    ca_certificate: Option<Certificate>,
    client_identity: Option<Identity>,
    accept_invalid_certs: bool,
}

impl ExternalIdentityTlsOptions {
    pub fn set_ca_certificate(&mut self, certificate: Certificate) {
        self.ca_certificate = Some(certificate);
    }

    pub fn set_client_identity(&mut self, identity: Identity) {
        self.client_identity = Some(identity);
    }

    pub fn set_accept_invalid_certs(&mut self, accept: bool) {
        self.accept_invalid_certs = accept;
    }
}

impl ExternalIdentityProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: Url,
        jwks_url: Url,
        auth_token: Option<String>,
        environment: String,
        instance_id: String,
        audience: String,
        app_version: String,
        jwks_refresh_seconds: u64,
        tls: ExternalIdentityTlsOptions,
        audit: Arc<dyn AuditSink>,
    ) -> Result<Self, IdentityError> {
        Ok(Self {
            base_url,
            jwks_url,
            auth_token,
            environment,
            instance_id,
            audience,
            app_version,
            audit,
            jwks_cache: RwLock::new(None),
            jwks_ttl: Duration::from_secs(jwks_refresh_seconds),
            tls,
        })
    }

    fn issue_token_internal(
        &self,
        request: IssueTokenRequest,
    ) -> Result<IssuedToken, IdentityError> {
        let IssueTokenRequest {
            actor,
            user_id,
            display_name,
            role,
        } = request;
        let url = self.join_path(TOKENS_ISSUE_PATH)?;
        let body = IssueTokenBody {
            user_id: user_id.as_str(),
            role: role.as_str(),
            display_name: display_name.as_deref(),
            environment: self.environment.as_str(),
            instance_id: self.instance_id.as_str(),
            app_version: self.app_version.as_str(),
        };
        let client = self.build_client()?;
        let response = self
            .send(client.post(url).json(&body))?
            .json::<IssueTokenResponse>()
            .map_err(|err| {
                IdentityError::Invalid(identity_messages::invalid_identity_response(
                    &err.to_string(),
                ))
            })?;

        let (header, raw_claims, _) = parse_jwt(&response.token)?;
        let claims: Claims = serde_json::from_slice(&raw_claims.decoded).map_err(|err| {
            IdentityError::Invalid(identity_messages::invalid_token_claims(&err.to_string()))
        })?;
        let issued_at = OffsetDateTime::from_unix_timestamp(claims.iat).map_err(|_| {
            IdentityError::Invalid(identity_messages::token_timestamp_out_of_range("issued-at"))
        })?;
        let expires_at = OffsetDateTime::from_unix_timestamp(claims.exp).map_err(|_| {
            IdentityError::Invalid(identity_messages::token_timestamp_out_of_range("expiry"))
        })?;
        let fingerprint = response
            .fingerprint
            .unwrap_or_else(|| fingerprint_token(&response.token));

        let issued = IssuedToken {
            token: response.token,
            issued_at,
            token_id: response.token_id.unwrap_or_else(|| claims.jti.clone()),
            user_id: user_id.clone(),
            expires_at,
            key_id: header.kid.clone(),
            fingerprint: fingerprint.clone(),
        };

        self.record_issue_audit(actor, &issued, &role);
        Ok(issued)
    }

    fn list_users_internal(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        let url = self.join_path(USERS_LIST_PATH)?;
        let client = self.build_client()?;
        let response = self
            .send(client.get(url))?
            .json::<ListUsersResponse>()
            .map_err(|err| {
                IdentityError::Invalid(identity_messages::invalid_identity_users_response(
                    &err.to_string(),
                ))
            })?;

        let users = response
            .users
            .into_iter()
            .filter_map(|user| match Role::from_str(&user.role) {
                Ok(role) => Some((user, role)),
                Err(err) => {
                    warn!(
                        user = %user.user_id,
                        error = %err,
                        "{}",
                        identity_messages::skipping_identity_user_invalid_role()
                    );
                    None
                }
            })
            .map(|(user, role)| IdentityUserRecord {
                user_id: user.user_id,
                display_name: user.display_name,
                role,
                created_at: user
                    .created_at
                    .and_then(|ts| parse_timestamp(&ts))
                    .unwrap_or_else(OffsetDateTime::now_utc),
                last_issued_at: user.last_issued_at.and_then(|ts| parse_timestamp(&ts)),
                token_count: user.token_count.unwrap_or(0),
                last_token_fingerprint: user.last_token_fingerprint,
                tokens: user
                    .tokens
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|token| IdentityTokenRecord::try_from(token).ok())
                    .collect(),
                password_hash: None,
                password_updated_at: user.password_updated_at.and_then(|ts| parse_timestamp(&ts)),
                last_login_at: user.last_login_at.and_then(|ts| parse_timestamp(&ts)),
            })
            .collect();
        Ok(users)
    }

    fn authenticate_internal(
        &self,
        user_id: &str,
        password: &str,
    ) -> Result<IdentityUserProfile, IdentityError> {
        let url = self.join_path(LOGIN_PATH)?;
        let client = self.build_client()?;
        let payload = LoginRequest { user_id, password };
        let response = self
            .send(client.post(url).json(&payload))?
            .json::<LoginResponse>()
            .map_err(|err| {
                IdentityError::Invalid(format!("invalid identity login response: {err}"))
            })?;
        let role = Role::from_str(&response.role)
            .map_err(|err| IdentityError::Invalid(format!("unknown role '{}'", err.value())))?;
        let last_login_at = response.last_login_at.as_deref().and_then(parse_timestamp);
        let password_updated_at = response
            .password_updated_at
            .as_deref()
            .and_then(parse_timestamp);
        Ok(IdentityUserProfile {
            user_id: response.user_id,
            display_name: response.display_name,
            role,
            password_updated_at,
            last_login_at,
        })
    }

    fn verify_internal(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        let (header, raw_claims, signature) = parse_jwt(token)?;
        let public_key = self.lookup_public_key(&header.kid)?;
        let signing_input = format!("{}.{}", header.encoded_header, raw_claims.encoded_claims);
        public_key
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| IdentityError::Unauthorized("signature verification failed".into()))?;
        let claims: Claims = serde_json::from_slice(&raw_claims.decoded)
            .map_err(|err| IdentityError::Unauthorized(format!("invalid claims: {err}")))?;
        self.validate_claims(&claims)?;
        Ok(IdentityClaims {
            user_id: claims.sub,
            role: Role::from_str(&claims.role)
                .map_err(|err| IdentityError::Invalid(format!("unknown role '{}'", err.value())))?,
            environment: claims.env,
            instance_id: claims.inst,
            app_version: claims.ver,
            issued_at: OffsetDateTime::from_unix_timestamp(claims.iat)
                .map_err(|_| IdentityError::Invalid("invalid issued-at timestamp".into()))?,
            expires_at: OffsetDateTime::from_unix_timestamp(claims.exp)
                .map_err(|_| IdentityError::Invalid("invalid expiry timestamp".into()))?,
            key_id: header.kid,
            token_id: claims.jti,
        })
    }

    fn validate_claims(&self, claims: &Claims) -> Result<(), IdentityError> {
        if claims.aud != self.audience {
            return Err(IdentityError::Unauthorized("audience mismatch".into()));
        }
        if claims.env != self.environment {
            return Err(IdentityError::Unauthorized("environment mismatch".into()));
        }
        if OffsetDateTime::now_utc().unix_timestamp() >= claims.exp {
            return Err(IdentityError::Unauthorized("token expired".into()));
        }
        Ok(())
    }

    fn record_issue_audit(&self, actor: AuditActor, issued: &IssuedToken, role: &Role) {
        let metadata = AuditMetadata::default()
            .insert("environment", self.environment.clone())
            .insert("instance", self.instance_id.clone())
            .insert("key_id", issued.key_id.clone())
            .insert("token_id", issued.token_id.clone())
            .insert("expires_at", issued.expires_at.to_string())
            .insert("issued_at", issued.issued_at.to_string())
            .insert("fingerprint", issued.fingerprint.clone())
            .insert("role", role.as_str())
            .insert("audience", self.audience.clone());
        let builder = AuditEvent::builder()
            .actor(actor)
            .action("identity.token.issue")
            .target(format!(
                "identity://{}/{}",
                self.environment, issued.user_id
            ))
            .metadata(metadata);
        match builder.build() {
            Ok(event) => {
                if let Err(err) = self.audit.record(event) {
                    warn!(error = %err, "failed to append identity audit event");
                }
            }
            Err(err) => warn!(error = %err, "failed to build identity audit event"),
        }
    }

    fn lookup_public_key(&self, kid: &str) -> Result<PublicKey, IdentityError> {
        if let Some(key) = self.cached_key(kid) {
            return Ok(key);
        }
        self.refresh_jwks()?;
        self.cached_key(kid)
            .ok_or_else(|| IdentityError::Unauthorized("unknown signing key".into()))
    }

    fn cached_key(&self, kid: &str) -> Option<PublicKey> {
        if let Ok(guard) = self.jwks_cache.read() {
            if let Some(cache) = guard.as_ref() {
                if cache.expires_at > Instant::now() {
                    return cache.keys.get(kid).cloned();
                }
            }
        }
        None
    }

    fn refresh_jwks(&self) -> Result<(), IdentityError> {
        let mut guard = self
            .jwks_cache
            .write()
            .map_err(|_| IdentityError::StatePoisoned)?;
        let client = self.build_client()?;
        let response = match self.send(client.get(self.jwks_url.clone())) {
            Ok(resp) => resp,
            Err(err) => {
                if guard.is_some() {
                    warn!(error = %err, "failed to refresh identity JWKS; using cached keys");
                    return Ok(());
                }
                return Err(err);
            }
        };
        let jwks: JwksDocument = response
            .json()
            .map_err(|err| IdentityError::Invalid(format!("invalid JWKS document: {err}")))?;
        if jwks.keys.is_empty() {
            return Err(IdentityError::Invalid(
                "identity JWKS document contained no keys".into(),
            ));
        }
        let mut map = HashMap::with_capacity(jwks.keys.len());
        for key in jwks.keys {
            if key.kty != "OKP" || !key.crv.eq_ignore_ascii_case("ED25519") {
                warn!(kid = %key.kid, "skipping unsupported JWKS key type");
                continue;
            }
            let bytes = URL_SAFE_NO_PAD.decode(key.x.as_bytes()).map_err(|err| {
                IdentityError::Invalid(format!("invalid JWKS key encoding: {err}"))
            })?;
            if bytes.len() != 32 {
                warn!(kid = %key.kid, "skipping JWKS key with unexpected length");
                continue;
            }
            let public = PublicKey::from_bytes(&bytes)
                .map_err(|_| IdentityError::Invalid("failed to parse JWKS public key".into()))?;
            map.insert(key.kid, public);
        }
        if map.is_empty() {
            return Err(IdentityError::Invalid(
                "no supported keys found in identity JWKS".into(),
            ));
        }
        let expires_at = Instant::now() + self.jwks_ttl;
        *guard = Some(JwksCache {
            keys: map,
            expires_at,
        });
        debug!("identity JWKS updated");
        Ok(())
    }

    fn build_client(&self) -> Result<Client, IdentityError> {
        let mut builder =
            Client::builder().user_agent(format!("fenrir-identity-client/{}", self.app_version));

        if let Some(cert) = self.tls.ca_certificate.clone() {
            builder = builder.add_root_certificate(cert);
        }

        if let Some(identity) = self.tls.client_identity.clone() {
            builder = builder.identity(identity);
        }

        if self.tls.accept_invalid_certs {
            builder = builder.danger_accept_invalid_certs(true);
        }

        builder.build().map_err(|err| {
            IdentityError::Invalid(format!("failed to build identity client: {err}"))
        })
    }

    fn send(&self, request: RequestBuilder) -> Result<reqwest::blocking::Response, IdentityError> {
        let request = if let Some(token) = &self.auth_token {
            request.bearer_auth(token)
        } else {
            request
        };
        request
            .send()
            .map_err(|err| IdentityError::Invalid(format!("identity server request failed: {err}")))
            .and_then(|response| {
                if response.status().is_success() {
                    Ok(response)
                } else {
                    Err(IdentityError::Unauthorized(format!(
                        "identity server returned status {}",
                        response.status()
                    )))
                }
            })
    }

    fn join_path(&self, path: &str) -> Result<Url, IdentityError> {
        self.base_url
            .join(path)
            .map_err(|err| IdentityError::Invalid(format!("invalid identity path: {err}")))
    }
}

impl IdentityProvider for ExternalIdentityProvider {
    fn issue_token(&self, request: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        self.issue_token_internal(request)
    }

    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        self.list_users_internal()
    }

    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        self.verify_internal(token)
    }

    fn authenticate_user(
        &self,
        user_id: &str,
        password: &str,
    ) -> Result<IdentityUserProfile, IdentityError> {
        self.authenticate_internal(user_id, password)
    }

    fn set_user_password(
        &self,
        _user_id: &str,
        _password_hash: &str,
        _role: crate::security::auth::Role,
    ) -> Result<(), IdentityError> {
        // External identity provider manages passwords externally
        Err(IdentityError::Invalid(
            "password management not supported for external identity provider".into(),
        ))
    }

    fn is_password_set(&self, _user_id: &str) -> Result<bool, IdentityError> {
        // External provider always assumes password is set (managed externally)
        Ok(true)
    }

    fn take_pending_password(&self) -> Option<String> {
        // External provider doesn't support pending passwords
        None
    }
}

#[derive(Serialize)]
struct IssueTokenBody<'a> {
    user_id: &'a str,
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
    environment: &'a str,
    instance_id: &'a str,
    app_version: &'a str,
}

#[derive(Deserialize)]
struct IssueTokenResponse {
    token: String,
    #[serde(default)]
    token_id: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
}

#[derive(Deserialize)]
struct ListUsersResponse {
    users: Vec<ListUserEntry>,
}

#[derive(Deserialize)]
struct ListUserEntry {
    user_id: String,
    role: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    last_issued_at: Option<String>,
    #[serde(default)]
    token_count: Option<u64>,
    #[serde(default)]
    last_token_fingerprint: Option<String>,
    #[serde(default)]
    tokens: Option<Vec<ListUserTokenEntry>>,
    #[serde(default)]
    password_updated_at: Option<String>,
    #[serde(default)]
    last_login_at: Option<String>,
}

#[derive(Deserialize)]
struct ListUserTokenEntry {
    token_id: String,
    fingerprint: String,
    #[serde(default)]
    issued_at: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    key_id: Option<String>,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    user_id: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct LoginResponse {
    user_id: String,
    #[serde(default)]
    display_name: Option<String>,
    role: String,
    #[serde(default)]
    last_login_at: Option<String>,
    #[serde(default)]
    password_updated_at: Option<String>,
}

impl TryFrom<ListUserTokenEntry> for IdentityTokenRecord {
    type Error = IdentityError;

    fn try_from(entry: ListUserTokenEntry) -> Result<Self, Self::Error> {
        let issued_at = entry
            .issued_at
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or_else(OffsetDateTime::now_utc);
        let expires_at = entry
            .expires_at
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or(issued_at);
        Ok(Self {
            token_id: entry.token_id,
            fingerprint: entry.fingerprint,
            issued_at,
            expires_at,
            key_id: entry.key_id.unwrap_or_default(),
        })
    }
}

#[derive(Deserialize)]
struct JwksDocument {
    keys: Vec<JwkEntry>,
}

#[derive(Deserialize)]
struct JwkEntry {
    kid: String,
    kty: String,
    crv: String,
    x: String,
}

fn parse_timestamp(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
}
