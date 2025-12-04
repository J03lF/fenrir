use std::{fmt, str::FromStr};

use serde::{Serialize, Serializer};

use crate::utils::messages::security::service as service_messages;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceRole {
    Admin,
    Write,
    Read,
}

impl ServiceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceRole::Admin => "service-admin",
            ServiceRole::Write => "service-write",
            ServiceRole::Read => "service-read",
        }
    }

    pub fn satisfies(&self, required: ServiceRole) -> bool {
        matches!(
            (self, required),
            (ServiceRole::Admin, _)
                | (ServiceRole::Write, ServiceRole::Write | ServiceRole::Read)
                | (ServiceRole::Read, ServiceRole::Read)
        )
    }

    pub const fn variants() -> &'static [&'static str; 3] {
        &["service-admin", "service-write", "service-read"]
    }
}

impl FromStr for ServiceRole {
    type Err = ServiceRoleParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "service-admin" => Ok(ServiceRole::Admin),
            "service-write" => Ok(ServiceRole::Write),
            "service-read" => Ok(ServiceRole::Read),
            other => Err(ServiceRoleParseError {
                value: other.to_string(),
                message: service_messages::unknown_service_role(other),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceRoleParseError {
    value: String,
    message: String,
}

impl ServiceRoleParseError {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ServiceRoleParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServiceRoleParseError {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServiceScope(String);

const MAX_SCOPE_LENGTH: usize = 96;

impl ServiceScope {
    pub fn new(value: impl Into<String>) -> Result<Self, ServiceScopeError> {
        let raw: String = value.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ServiceScopeError {
                value: raw,
                message: service_messages::scope_empty().to_string(),
            });
        }
        if trimmed.len() > MAX_SCOPE_LENGTH {
            return Err(ServiceScopeError {
                value: trimmed.to_string(),
                message: service_messages::scope_too_long(MAX_SCOPE_LENGTH),
            });
        }
        let mut parts = trimmed.splitn(2, ':');
        let Some(domain) = parts.next() else {
            return Err(ServiceScopeError {
                value: trimmed.to_string(),
                message: service_messages::invalid_scope(trimmed),
            });
        };
        let Some(action) = parts.next() else {
            return Err(ServiceScopeError {
                value: trimmed.to_string(),
                message: service_messages::invalid_scope(trimmed),
            });
        };
        if !is_valid_segment(domain) || !is_valid_segment(action) {
            return Err(ServiceScopeError {
                value: trimmed.to_string(),
                message: service_messages::scope_invalid_segment().to_string(),
            });
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ServiceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl FromStr for ServiceScope {
    type Err = ServiceScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        ServiceScope::new(value)
    }
}

impl TryFrom<&str> for ServiceScope {
    type Error = ServiceScopeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        ServiceScope::new(value)
    }
}

fn is_valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_' ))
}

#[derive(Debug, Clone)]
pub struct ServiceScopeError {
    value: String,
    message: String,
}

impl ServiceScopeError {
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ServiceScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ServiceScopeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_role_parsing_and_ordering() {
        assert!(ServiceRole::Admin.satisfies(ServiceRole::Write));
        assert!(ServiceRole::Write.satisfies(ServiceRole::Read));
        assert!(!ServiceRole::Read.satisfies(ServiceRole::Write));
        assert_eq!(
            ServiceRole::from_str("service-admin").unwrap(),
            ServiceRole::Admin
        );
        assert!(ServiceRole::from_str("invalid").is_err());
    }

    #[test]
    fn service_scope_validations() {
        let scope = ServiceScope::new("tickets:read").expect("valid scope");
        assert_eq!(scope.as_str(), "tickets:read");
        assert!(ServiceScope::new("").is_err());
        assert!(ServiceScope::new("tickets").is_err());
        assert!(ServiceScope::new("Tickets:Read").is_err());
    }
}
