use std::sync::Arc;

use subtle::ConstantTimeEq;

use crate::security::auth::{AuthError, Role};
use crate::security::identity::IdentityProvider;

pub struct ControlPlaneAuthorizer {
    backend: ControlPlaneBackend,
}

enum ControlPlaneBackend {
    Identity(Arc<dyn IdentityProvider>),
    Static(Vec<TokenEntry>),
    Disabled,
}

struct TokenEntry {
    role: Role,
    secret: String,
}

impl ControlPlaneAuthorizer {
    pub fn with_identity(identity: Arc<dyn IdentityProvider>) -> Self {
        Self {
            backend: ControlPlaneBackend::Identity(identity),
        }
    }

    pub fn with_tokens(entries: Vec<(Role, String)>) -> Self {
        if entries.is_empty() {
            return Self {
                backend: ControlPlaneBackend::Disabled,
            };
        }
        let tokens = entries
            .into_iter()
            .map(|(role, secret)| TokenEntry { role, secret })
            .collect();
        Self {
            backend: ControlPlaneBackend::Static(tokens),
        }
    }

    pub fn is_configured(&self) -> bool {
        match &self.backend {
            ControlPlaneBackend::Identity(_) => true,
            ControlPlaneBackend::Static(tokens) => !tokens.is_empty(),
            ControlPlaneBackend::Disabled => false,
        }
    }

    pub fn authorize_token(&self, bearer: Option<&str>) -> Result<Role, AuthError> {
        match &self.backend {
            ControlPlaneBackend::Identity(identity) => {
                let token = bearer.ok_or(AuthError::Unauthorized)?.trim().to_owned();
                let identity = Arc::clone(identity);
                let claims = tokio::task::block_in_place(|| identity.verify(&token))
                    .map_err(AuthError::from)?;
                Ok(claims.role)
            }
            ControlPlaneBackend::Static(tokens) => {
                if tokens.is_empty() {
                    return Err(AuthError::Unauthorized);
                }
                let token = bearer.ok_or(AuthError::Unauthorized)?.trim();
                for entry in tokens {
                    if entry.secret.as_bytes().ct_eq(token.as_bytes()).into() {
                        return Ok(entry.role);
                    }
                }
                Err(AuthError::Forbidden)
            }
            ControlPlaneBackend::Disabled => Err(AuthError::Unauthorized),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
