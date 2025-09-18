#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
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
