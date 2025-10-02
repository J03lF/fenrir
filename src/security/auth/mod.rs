pub mod http;

pub use http::ControlPlaneAuthorizer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    pub fn satisfies(&self, required: Role) -> bool {
        matches!(
            (self, required),
            (Role::Admin, _)
                | (Role::Operator, Role::Operator | Role::Viewer)
                | (Role::Viewer, Role::Viewer)
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Viewer => "viewer",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub user_id: String,
    pub role: Role,
}

pub trait Authenticator {
    fn login(&self, username: &str, password: &str) -> Result<Session, AuthError>;
}

pub trait RbacGate {
    fn check(&self, session: &Session, required: Role) -> Result<(), AuthError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
}
