use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{PublicKey, Verifier};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::{debug, warn};

use super::{
    fingerprint_token, parse_jwt, Claims, IdentityClaims, IdentityError, IdentityProvider,
    IdentityUserRecord, IssueTokenRequest, IssuedToken,
};
use crate::audit::{AuditActor, AuditEvent, AuditMetadata};
use crate::security::auth::Role;
use crate::security::manager::AuditSink;

const TOKENS_ISSUE_PATH: &str = "tokens/issue";
const USERS_LIST_PATH: &str = "users";

pub struct ExternalIdentityProvider {
    client: Client,
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
}

struct JwksCache {
    keys: HashMap<String, PublicKey>,
    expires_at: Instant,
}

impl ExternalIdentityProvider {
    pub fn new(
        base_url: Url,
        jwks_url: Url,
        auth_token: Option<String>,
        environment: String,
        instance_id: String,
        audience: String,
        app_version: String,
        jwks_refresh_seconds: u64,
        audit: Arc<dyn AuditSink>,
    ) -> Result<Self, IdentityError> {
        let client = Client::builder()
            .user_agent(format!("fenrir-identity-client/{}", app_version))
            .build()
            .map_err(|err| {
                IdentityError::Invalid(format!("failed to build identity client: {err}"))
            })?;
        Ok(Self {
            client,
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
        let response = self
            .send(self.client.post(url).json(&body))?
            .json::<IssueTokenResponse>()
            .map_err(|err| IdentityError::Invalid(format!("invalid identity response: {err}")))?;

        let (header, raw_claims, _) = parse_jwt(&response.token)?;
        let claims: Claims = serde_json::from_slice(&raw_claims.decoded)
            .map_err(|err| IdentityError::Invalid(format!("invalid token claims: {err}")))?;
        let expires_at = OffsetDateTime::from_unix_timestamp(claims.exp)
            .map_err(|_| IdentityError::Invalid("token expiry timestamp out of range".into()))?;
        let fingerprint = response
            .fingerprint
            .unwrap_or_else(|| fingerprint_token(&response.token));

        let issued = IssuedToken {
            token: response.token,
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
        let response = self
            .send(self.client.get(url))?
            .json::<ListUsersResponse>()
            .map_err(|err| {
                IdentityError::Invalid(format!("invalid identity users response: {err}"))
            })?;

        let users = response
            .users
            .into_iter()
            .filter_map(|user| match parse_role(&user.role) {
                Ok(role) => Some((user, role)),
                Err(err) => {
                    warn!(user = %user.user_id, error = %err, "skipping identity user with invalid role");
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
            })
            .collect();
        Ok(users)
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
            role: parse_role(&claims.role)?,
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
        let response = match self.send(self.client.get(self.jwks_url.clone())) {
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
            if key.kty != "OKP" || key.crv.to_ascii_uppercase() != "ED25519" {
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

fn parse_role(value: &str) -> Result<Role, IdentityError> {
    match value.to_ascii_lowercase().as_str() {
        "admin" => Ok(Role::Admin),
        "operator" => Ok(Role::Operator),
        "viewer" => Ok(Role::Viewer),
        other => Err(IdentityError::Invalid(format!("unknown role '{other}'"))),
    }
}
