use super::error::AuthError;
use super::role::Role;
use super::session::Session;

pub trait Authenticator {
    fn login(&self, username: &str, password: &str) -> Result<Session, AuthError>;
}

pub trait RbacGate {
    fn check(&self, session: &Session, required: Role) -> Result<(), AuthError>;
}
