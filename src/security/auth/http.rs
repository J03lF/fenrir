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
#[path = "../../../tests/unit/security/auth/http_tests.rs"]
mod tests;
