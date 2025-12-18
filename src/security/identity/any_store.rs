//! Unified store wrapper that supports both file and database backends.

use crate::security::auth::Role;

use super::db_store::DbIdentityStore;
use super::errors::IdentityError;
use super::store::{IdentityState, IdentityStore, IdentityUserRecord};

/// Enum wrapper for identity storage backends.
/// This allows `IdentityAuthority` to work with either file or database storage.
pub enum AnyIdentityStore {
    File(IdentityStore),
    Db(DbIdentityStore),
}

impl AnyIdentityStore {
    /// Read state through a callback
    pub fn read<F, R>(&self, reader: F) -> Result<R, IdentityError>
    where
        F: FnOnce(&IdentityState) -> R,
    {
        match self {
            AnyIdentityStore::File(store) => store.read(reader),
            AnyIdentityStore::Db(store) => {
                let state = store.load_state()?;
                Ok(reader(&state))
            }
        }
    }

    /// Write state through a callback
    pub fn write<F, R>(&self, writer: F) -> Result<R, IdentityError>
    where
        F: FnOnce(&mut IdentityState) -> Result<R, IdentityError>,
    {
        match self {
            AnyIdentityStore::File(store) => store.write(writer),
            AnyIdentityStore::Db(store) => {
                // For DB store, we load state, modify it, then persist changes
                // Note: Individual operations are already persisted in DbIdentityStore
                let mut state = store.load_state()?;
                let result = writer(&mut state)?;
                // The DbIdentityStore persists on each operation, so no explicit persist needed
                Ok(result)
            }
        }
    }

    /// Check if password is set for a user
    pub fn is_password_set(&self, user_id: &str) -> Result<bool, IdentityError> {
        match self {
            AnyIdentityStore::File(store) => store.is_password_set(user_id),
            AnyIdentityStore::Db(store) => store.is_password_set(user_id),
        }
    }

    /// Get user record
    pub fn get_user(&self, user_id: &str) -> Result<Option<IdentityUserRecord>, IdentityError> {
        match self {
            AnyIdentityStore::File(store) => store.get_user(user_id),
            AnyIdentityStore::Db(store) => store.get_user(user_id),
        }
    }

    /// Set password for a user
    pub fn set_password(
        &self,
        user_id: &str,
        password_hash: String,
        role: Role,
    ) -> Result<(), IdentityError> {
        match self {
            AnyIdentityStore::File(store) => store.set_password(user_id, password_hash, role),
            AnyIdentityStore::Db(store) => store.set_password(user_id, password_hash, role),
        }
    }

    /// Record login timestamp
    pub fn record_login(&self, user_id: &str) -> Result<(), IdentityError> {
        match self {
            AnyIdentityStore::File(store) => store.record_login(user_id),
            AnyIdentityStore::Db(store) => store.record_login(user_id),
        }
    }

    /// Persist state (no-op for DB store)
    pub fn persist(&self) -> Result<(), IdentityError> {
        match self {
            AnyIdentityStore::File(store) => store.persist(),
            AnyIdentityStore::Db(store) => store.persist(),
        }
    }

    /// Take pending password from setup script
    pub fn take_pending_password(&self) -> Option<String> {
        match self {
            AnyIdentityStore::File(store) => store.take_pending_password(),
            AnyIdentityStore::Db(store) => store.take_pending_password(),
        }
    }
}

