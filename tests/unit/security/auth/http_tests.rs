use std::sync::Arc;

use time::OffsetDateTime;

use crate::security::identity::{
    IdentityClaims, IdentityError, IdentityProvider, IdentityUserProfile, IdentityUserRecord,
    IssueTokenRequest, IssuedToken,
};

use super::*;

#[test]
fn authorizes_matching_token() {
    let authorizer = ControlPlaneAuthorizer::with_tokens(vec![(Role::Admin, "token".into())]);
    let role = authorizer
        .authorize_token(Some("token"))
        .expect("authorized");
    assert_eq!(role, Role::Admin);
}

#[test]
fn rejects_mismatched_token() {
    let authorizer = ControlPlaneAuthorizer::with_tokens(vec![(Role::Admin, "token".into())]);
    assert!(matches!(
        authorizer.authorize_token(Some("nope")),
        Err(AuthError::Forbidden)
    ));
}

#[test]
fn rejects_missing_token_when_configured() {
    let authorizer = ControlPlaneAuthorizer::with_tokens(vec![(Role::Admin, "token".into())]);
    assert!(matches!(
        authorizer.authorize_token(None),
        Err(AuthError::Unauthorized)
    ));
}

#[test]
fn falls_back_to_static_token_when_identity_rejects() {
    let identity = Arc::new(RejectIdentity {});
    let authorizer = ControlPlaneAuthorizer::with_identity_and_tokens(
        identity,
        vec![(Role::Admin, "static".into())],
    );
    let role = authorizer
        .authorize_token(Some("static"))
        .expect("authorized via static token");
    assert_eq!(role, Role::Admin);
}

#[test]
fn prefers_identity_token_even_when_static_tokens_exist() {
    let identity = Arc::new(AcceptIdentity {
        expected: "identity-token",
        role: Role::Operator,
    });
    let authorizer = ControlPlaneAuthorizer::with_identity_and_tokens(
        identity,
        vec![(Role::Admin, "static".into())],
    );
    let role = authorizer
        .authorize_token(Some("identity-token"))
        .expect("authorized via identity");
    assert_eq!(role, Role::Operator);
}

struct RejectIdentity;

impl IdentityProvider for RejectIdentity {
    fn issue_token(&self, _: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        unimplemented!()
    }

    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        unimplemented!()
    }

    fn verify(&self, _: &str) -> Result<IdentityClaims, IdentityError> {
        Err(IdentityError::Unauthorized("rejected".into()))
    }

    fn authenticate_user(&self, _: &str, _: &str) -> Result<IdentityUserProfile, IdentityError> {
        unimplemented!()
    }

    fn set_user_password(&self, _: &str, _: &str, _: Role) -> Result<(), IdentityError> {
        unimplemented!()
    }

    fn is_password_set(&self, _: &str) -> Result<bool, IdentityError> {
        Ok(true)
    }

    fn take_pending_password(&self) -> Option<String> {
        None
    }
}

struct AcceptIdentity {
    expected: &'static str,
    role: Role,
}

impl IdentityProvider for AcceptIdentity {
    fn issue_token(&self, _: IssueTokenRequest) -> Result<IssuedToken, IdentityError> {
        unimplemented!()
    }

    fn list_users(&self) -> Result<Vec<IdentityUserRecord>, IdentityError> {
        unimplemented!()
    }

    fn verify(&self, token: &str) -> Result<IdentityClaims, IdentityError> {
        if token == self.expected {
            Ok(sample_claims(self.role))
        } else {
            Err(IdentityError::Unauthorized("invalid".into()))
        }
    }

    fn authenticate_user(&self, _: &str, _: &str) -> Result<IdentityUserProfile, IdentityError> {
        unimplemented!()
    }

    fn set_user_password(&self, _: &str, _: &str, _: Role) -> Result<(), IdentityError> {
        unimplemented!()
    }

    fn is_password_set(&self, _: &str) -> Result<bool, IdentityError> {
        Ok(true)
    }

    fn take_pending_password(&self) -> Option<String> {
        None
    }
}

fn sample_claims(role: Role) -> IdentityClaims {
    let now = OffsetDateTime::now_utc();
    IdentityClaims {
        user_id: "user".into(),
        role,
        environment: "test".into(),
        instance_id: "test".into(),
        app_version: "0.0.0".into(),
        issued_at: now,
        expires_at: now,
        key_id: "test".into(),
        token_id: "test".into(),
    }
}
