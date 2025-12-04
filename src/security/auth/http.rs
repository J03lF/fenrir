use std::sync::Arc;

use subtle::ConstantTimeEq;

use crate::security::auth::{AuthError, Role};
use crate::security::identity::IdentityProvider;

pub struct ControlPlaneAuthorizer {
    identity: Option<Arc<dyn IdentityProvider>>,
    tokens: Vec<TokenEntry>,
}

struct TokenEntry {
    role: Role,
    secret: String,
}

impl ControlPlaneAuthorizer {
    pub fn with_identity(identity: Arc<dyn IdentityProvider>) -> Self {
        Self::new(Some(identity), Vec::new())
    }

    pub fn with_tokens(entries: Vec<(Role, String)>) -> Self {
        Self::new(None, entries)
    }

    pub fn with_identity_and_tokens(
        identity: Arc<dyn IdentityProvider>,
        entries: Vec<(Role, String)>,
    ) -> Self {
        Self::new(Some(identity), entries)
    }

    fn new(identity: Option<Arc<dyn IdentityProvider>>, entries: Vec<(Role, String)>) -> Self {
        let tokens = entries
            .into_iter()
            .map(|(role, secret)| TokenEntry { role, secret })
            .collect();
        Self { identity, tokens }
    }

    pub fn is_configured(&self) -> bool {
        self.identity.is_some() || !self.tokens.is_empty()
    }

    pub fn authorize_token(&self, bearer: Option<&str>) -> Result<Role, AuthError> {
        let token = bearer.ok_or(AuthError::Unauthorized)?.trim();
        let mut last_err = None;

        if let Some(identity) = &self.identity {
            match tokio::task::block_in_place(|| identity.verify(token)) {
                Ok(claims) => return Ok(claims.role),
                Err(err) => {
                    last_err = Some(AuthError::from(err));
                }
            }
        }

        if self.tokens.is_empty() {
            return Err(last_err.unwrap_or(AuthError::Unauthorized));
        }

        for entry in &self.tokens {
            if entry.secret.as_bytes().ct_eq(token.as_bytes()).into() {
                return Ok(entry.role);
            }
        }

        Err(last_err.unwrap_or(AuthError::Forbidden))
    }
}

#[cfg(test)]
mod tests {
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

        fn authenticate_user(
            &self,
            _: &str,
            _: &str,
        ) -> Result<IdentityUserProfile, IdentityError> {
            unimplemented!()
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

        fn authenticate_user(
            &self,
            _: &str,
            _: &str,
        ) -> Result<IdentityUserProfile, IdentityError> {
            unimplemented!()
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
}
