use std::sync::Arc;

use crate::domain::user::{
    EmailAddress, User, UserError, UserFilter, UserId, UserRepository, UserResult, UserRole,
};

#[derive(Clone)]
pub struct UserService {
    repo: Arc<dyn UserRepository>,
}

impl UserService {
    pub fn new(repo: Arc<dyn UserRepository>) -> Self {
        Self { repo }
    }

    pub fn register(&self, cmd: RegisterUserCommand) -> UserResult<User> {
        let email = EmailAddress::parse(cmd.email)?;
        if let Some(existing) = self.repo.find_by_username(&cmd.username)? {
            return Err(UserError::DuplicateUsername(existing.username));
        }
        if let Some(existing) = self.repo.find_by_email(&email)? {
            return Err(UserError::DuplicateEmail(existing.email.to_string()));
        }

        let user = User::new(
            UserId::new(),
            cmd.username,
            email,
            cmd.display_name,
            cmd.roles,
        )?;
        self.repo.insert(user)
    }

    pub fn update_roles(&self, user_id: &UserId, roles: Vec<UserRole>) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_roles(roles)?;
        self.repo.update(user)
    }

    pub fn set_lock_state(&self, user_id: &UserId, locked: bool) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_locked(locked);
        self.repo.update(user)
    }

    pub fn update_display_name(
        &self,
        user_id: &UserId,
        display_name: Option<String>,
    ) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_display_name(display_name);
        self.repo.update(user)
    }

    pub fn find_by_username(&self, username: &str) -> UserResult<Option<User>> {
        self.repo.find_by_username(username)
    }

    pub fn find_by_id(&self, user_id: &UserId) -> UserResult<Option<User>> {
        self.repo.find_by_id(user_id)
    }

    pub fn list(&self, filter: UserFilter) -> UserResult<Vec<User>> {
        self.repo.list(&filter)
    }
}

pub struct RegisterUserCommand {
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub roles: Vec<UserRole>,
}

impl RegisterUserCommand {
    pub fn new(
        username: impl Into<String>,
        email: impl Into<String>,
        display_name: Option<String>,
        roles: Vec<UserRole>,
    ) -> Self {
        Self {
            username: username.into(),
            email: email.into(),
            display_name: display_name.map(|s| s.trim().to_string()),
            roles,
        }
    }
}
