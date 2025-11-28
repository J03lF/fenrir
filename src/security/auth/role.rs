use std::{fmt, str::FromStr};

use crate::utils::messages::security::auth as auth_messages;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    pub const fn variants() -> &'static [&'static str; 3] {
        &["admin", "operator", "viewer"]
    }

    pub fn parse(value: &str) -> Result<Self, RoleParseError> {
        Role::from_str(value)
    }
}

impl FromStr for Role {
    type Err = RoleParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim();
        if normalized.eq_ignore_ascii_case("admin") {
            Ok(Role::Admin)
        } else if normalized.eq_ignore_ascii_case("operator") {
            Ok(Role::Operator)
        } else if normalized.eq_ignore_ascii_case("viewer") {
            Ok(Role::Viewer)
        } else {
            Err(RoleParseError {
                value: normalized.to_string(),
            })
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleParseError {
    value: String,
}

impl RoleParseError {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for RoleParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&auth_messages::unknown_role(&self.value))
    }
}

impl std::error::Error for RoleParseError {}
