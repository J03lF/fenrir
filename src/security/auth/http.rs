use crate::security::auth::{AuthError, Role};
use subtle::ConstantTimeEq;

pub struct ControlPlaneAuthorizer {
    tokens: Vec<TokenEntry>,
}

struct TokenEntry {
    role: Role,
    secret: String,
}

impl ControlPlaneAuthorizer {
    pub fn new(entries: Vec<(Role, String)>) -> Self {
        let tokens = entries
            .into_iter()
            .map(|(role, secret)| TokenEntry { role, secret })
            .collect();
        Self { tokens }
    }

    pub fn is_configured(&self) -> bool {
        !self.tokens.is_empty()
    }

    pub fn authorize_token(&self, bearer: Option<&str>) -> Result<Role, AuthError> {
        if self.tokens.is_empty() {
            return Err(AuthError::Unauthorized);
        }
        let token = bearer.ok_or(AuthError::Unauthorized)?.trim();
        for entry in &self.tokens {
            if entry.secret.as_bytes().ct_eq(token.as_bytes()).into() {
                return Ok(entry.role.clone());
            }
        }
        Err(AuthError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorizes_matching_token() {
        let authorizer = ControlPlaneAuthorizer::new(vec![(Role::Admin, "token".into())]);
        let role = authorizer
            .authorize_token(Some("token"))
            .expect("authorized");
        assert_eq!(role, Role::Admin);
    }

    #[test]
    fn rejects_mismatched_token() {
        let authorizer = ControlPlaneAuthorizer::new(vec![(Role::Admin, "token".into())]);
        assert!(matches!(
            authorizer.authorize_token(Some("nope")),
            Err(AuthError::Forbidden)
        ));
    }

    #[test]
    fn rejects_missing_token_when_configured() {
        let authorizer = ControlPlaneAuthorizer::new(vec![(Role::Admin, "token".into())]);
        assert!(matches!(
            authorizer.authorize_token(None),
            Err(AuthError::Unauthorized)
        ));
    }
}
