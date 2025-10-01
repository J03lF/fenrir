use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use uuid::Uuid;

pub type UserResult<T> = Result<T, UserError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UserId(Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for UserId {
    type Err = UserError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|_| UserError::Validation("ungültige User-ID".to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UserRole {
    Admin,
    Operator,
    Viewer,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::Admin => "admin",
            UserRole::Operator => "operator",
            UserRole::Viewer => "viewer",
        }
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for UserRole {
    type Err = UserError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "admin" => Ok(UserRole::Admin),
            "operator" => Ok(UserRole::Operator),
            "viewer" => Ok(UserRole::Viewer),
            other => Err(UserError::Validation(format!("unbekannte Rolle: {other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EmailAddress(String);

impl EmailAddress {
    pub fn parse(value: impl AsRef<str>) -> UserResult<Self> {
        let trimmed = value.as_ref().trim();
        if trimmed.is_empty() {
            return Err(UserError::Validation(
                "E-Mail-Adresse darf nicht leer sein".to_string(),
            ));
        }
        let parts: Vec<&str> = trimmed.split('@').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() || !parts[1].contains('.')
        {
            return Err(UserError::Validation(
                "E-Mail-Adresse hat kein gültiges Format".to_string(),
            ));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EmailAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: EmailAddress,
    pub display_name: Option<String>,
    pub roles: Vec<UserRole>,
    pub is_locked: bool,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl User {
    pub fn new(
        id: UserId,
        username: impl Into<String>,
        email: EmailAddress,
        display_name: Option<String>,
        roles: Vec<UserRole>,
    ) -> UserResult<Self> {
        let username = username.into();
        if username.trim().is_empty() {
            return Err(UserError::Validation(
                "Benutzername darf nicht leer sein".to_string(),
            ));
        }
        if roles.is_empty() {
            return Err(UserError::Validation(
                "Mindestens eine Rolle erforderlich".to_string(),
            ));
        }
        let created_at = SystemTime::now();
        Ok(Self {
            id,
            username,
            email,
            display_name: display_name.filter(|s| !s.trim().is_empty()),
            roles,
            is_locked: false,
            created_at,
            updated_at: created_at,
        })
    }

    pub fn touch(&mut self) {
        self.updated_at = SystemTime::now();
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.is_locked = locked;
        self.touch();
    }

    pub fn set_display_name(&mut self, display_name: Option<String>) {
        self.display_name = display_name.filter(|s| !s.trim().is_empty());
        self.touch();
    }

    pub fn set_roles(&mut self, roles: Vec<UserRole>) -> UserResult<()> {
        if roles.is_empty() {
            return Err(UserError::Validation(
                "Mindestens eine Rolle erforderlich".to_string(),
            ));
        }
        self.roles = roles;
        self.touch();
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserFilter {
    pub role: Option<UserRole>,
    pub search: Option<String>,
    pub include_locked: bool,
}

pub trait UserRepository: Send + Sync {
    fn insert(&self, user: User) -> UserResult<User>;
    fn update(&self, user: User) -> UserResult<User>;
    fn find_by_id(&self, id: &UserId) -> UserResult<Option<User>>;
    fn find_by_username(&self, username: &str) -> UserResult<Option<User>>;
    fn find_by_email(&self, email: &EmailAddress) -> UserResult<Option<User>>;
    fn list(&self, filter: &UserFilter) -> UserResult<Vec<User>>;
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum UserError {
    #[error("Benutzer nicht gefunden")]
    NotFound,
    #[error("Benutzername bereits vergeben: {0}")]
    DuplicateUsername(String),
    #[error("E-Mail bereits vergeben: {0}")]
    DuplicateEmail(String),
    #[error("Validierungsfehler: {0}")]
    Validation(String),
    #[error("Speicherfehler: {0}")]
    Storage(String),
}

impl UserError {
    pub fn storage<E: fmt::Display>(err: E) -> Self {
        UserError::Storage(err.to_string())
    }
}
