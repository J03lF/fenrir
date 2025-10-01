use std::sync::Arc;

use crate::domain::user::{
    EmailAddress, User, UserError, UserFilter, UserId, UserRepository, UserResult, UserRole,
};
use crate::infra::logging;
use tracing::info;

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
        let created = self.repo.insert(user)?;
        info!(user_id = %created.id, username = %created.username, "user registered");
        logging::append_db_log(&format!(
            "INSERT users id={} username={} roles={:?}",
            created.id, created.username, created.roles
        ));
        Ok(created)
    }

    pub fn update_roles(&self, user_id: &UserId, roles: Vec<UserRole>) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_roles(roles)?;
        let updated = self.repo.update(user)?;
        info!(user_id = %updated.id, roles = ?updated.roles.iter().map(UserRole::as_str).collect::<Vec<_>>(), "user roles updated");
        logging::append_db_log(&format!(
            "UPDATE users id={} set roles={:?}",
            updated.id, updated.roles
        ));
        Ok(updated)
    }

    pub fn set_lock_state(&self, user_id: &UserId, locked: bool) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_locked(locked);
        let updated = self.repo.update(user)?;
        info!(user_id = %updated.id, locked = locked, "user lock state updated");
        logging::append_db_log(&format!(
            "UPDATE users id={} set locked={}",
            updated.id, updated.is_locked
        ));
        Ok(updated)
    }

    pub fn update_display_name(
        &self,
        user_id: &UserId,
        display_name: Option<String>,
    ) -> UserResult<User> {
        let mut user = self.repo.find_by_id(user_id)?.ok_or(UserError::NotFound)?;
        user.set_display_name(display_name);
        let updated = self.repo.update(user)?;
        info!(user_id = %updated.id, "user display name updated");
        logging::append_db_log(&format!(
            "UPDATE users id={} set display_name={:?}",
            updated.id, updated.display_name
        ));
        Ok(updated)
    }

    pub fn find_by_username(&self, username: &str) -> UserResult<Option<User>> {
        logging::append_db_log(&format!("SELECT user by username={}", username));
        self.repo.find_by_username(username)
    }

    pub fn find_by_id(&self, user_id: &UserId) -> UserResult<Option<User>> {
        logging::append_db_log(&format!("SELECT user by id={}", user_id));
        self.repo.find_by_id(user_id)
    }

    pub fn list(&self, filter: UserFilter) -> UserResult<Vec<User>> {
        logging::append_db_log(&format!("SELECT users filter={:?}", filter));
        let result = self.repo.list(&filter);
        if let Ok(ref users) = result {
            info!(count = users.len(), "user list returned");
        }
        result
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
