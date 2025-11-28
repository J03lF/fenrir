use time::OffsetDateTime;

use crate::audit::AuditActor;
use crate::security::auth::Role;

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
    pub issued_at: OffsetDateTime,
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

#[derive(Clone, Debug)]
pub struct IdentityUserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub role: Role,
    pub password_updated_at: Option<OffsetDateTime>,
    pub last_login_at: Option<OffsetDateTime>,
}
