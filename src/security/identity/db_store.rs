use std::sync::Arc;

use base64::Engine;
use ed25519_dalek::{PublicKey, SecretKey};
use rand::rngs::OsRng;
use rand::RngCore;
use time::OffsetDateTime;
use tokio::runtime::Handle;
use tokio::task;
use uuid::Uuid;

use crate::domain::db::{DbError, DbExecutionResult};
use crate::security::auth::Role;
use crate::services::db_shell::DbShellService;
use crate::utils::messages::security::identity as identity_messages;

use super::store::{IdentityKeyMaterial, IdentityState, IdentityTokenRecord, IdentityUserRecord};
use super::IdentityError;

/// Database-backed identity store
pub struct DbIdentityStore {
    db_shell: Arc<DbShellService>,
    environment: String,
    instance_id: String,
    app_version: String,
    /// Path to check for pending password file (from setup script)
    pending_password_dir: Option<std::path::PathBuf>,
}

impl DbIdentityStore {
    pub fn new(
        db_shell: Arc<DbShellService>,
        environment: String,
        instance_id: String,
        app_version: String,
    ) -> Result<Self, IdentityError> {
        Ok(Self {
            db_shell,
            environment,
            instance_id,
            app_version,
            pending_password_dir: None,
        })
    }

    /// Set the directory where to look for pending password file from setup script
    pub fn with_pending_password_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.pending_password_dir = Some(dir);
        self
    }

    /// Initialize the store - creates key if none exists (async version)
    pub async fn initialize_async(&self) -> Result<(), IdentityError> {
        let existing_key = self.get_current_key_async().await?;
        if existing_key.is_none() {
            let mut secret_bytes = [0u8; 32];
            OsRng.fill_bytes(&mut secret_bytes);
            let secret = SecretKey::from_bytes(&secret_bytes).map_err(|_| {
                IdentityError::Invalid(identity_messages::unable_to_initialize_secret_key().into())
            })?;
            let public: PublicKey = (&secret).into();
            let key_id = format!("kid-{}", Uuid::new_v4());
            let now = OffsetDateTime::now_utc();

            self.insert_key_async(&key_id, &secret_bytes, public.as_bytes(), now, true)
                .await?;
        }
        Ok(())
    }

    /// Initialize (blocking wrapper)
    pub fn initialize(&self) -> Result<(), IdentityError> {
        block_on(self.initialize_async())
    }

    pub fn is_password_set(&self, user_id: &str) -> Result<bool, IdentityError> {
        block_on(self.is_password_set_async(user_id))
    }

    pub fn get_user(&self, user_id: &str) -> Result<Option<IdentityUserRecord>, IdentityError> {
        block_on(self.get_user_async(user_id))
    }

    pub fn set_password(
        &self,
        user_id: &str,
        password_hash: String,
        role: Role,
    ) -> Result<(), IdentityError> {
        block_on(self.set_password_async(user_id, password_hash, role))
    }

    pub fn record_login(&self, user_id: &str) -> Result<(), IdentityError> {
        block_on(self.record_login_async(user_id))
    }

    pub fn get_current_key(&self) -> Result<Option<IdentityKeyMaterial>, IdentityError> {
        block_on(self.get_current_key_async())
    }

    pub fn load_state(&self) -> Result<IdentityState, IdentityError> {
        block_on(self.load_state_async())
    }

    pub fn persist(&self) -> Result<(), IdentityError> {
        // No-op for DB store - data is persisted on each operation
        Ok(())
    }

    pub fn take_pending_password(&self) -> Option<String> {
        let pending_path = self
            .pending_password_dir
            .as_ref()?
            .join(".pending-password");

        if !pending_path.exists() {
            return None;
        }

        // Read and delete the pending password file
        match std::fs::read_to_string(&pending_path) {
            Ok(password) => {
                // Delete the file immediately for security
                let _ = std::fs::remove_file(&pending_path);
                let trimmed = password.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            Err(_) => {
                let _ = std::fs::remove_file(&pending_path);
                None
            }
        }
    }

    // === Async implementations ===

    async fn load_state_async(&self) -> Result<IdentityState, IdentityError> {
        let current_key = self
            .get_current_key_async()
            .await?
            .ok_or_else(|| IdentityError::Invalid("No current key found".into()))?;

        let users = self.load_all_users_async().await?;

        Ok(IdentityState {
            environment: self.environment.clone(),
            instance_id: self.instance_id.clone(),
            app_version: self.app_version.clone(),
            current_key,
            users,
        })
    }

    async fn get_current_key_async(&self) -> Result<Option<IdentityKeyMaterial>, IdentityError> {
        let session = self.db_shell.create_session();
        let query = "SELECT key_id, secret_key, public_key, created_at FROM identity_keys WHERE is_current = TRUE LIMIT 1";

        let results = session
            .simple_query(query)
            .await
            .map_err(db_to_identity_error)?;

        if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
            if let Some(row) = rs.rows.first() {
                return Ok(Some(self.parse_key_row(row)?));
            }
        }
        Ok(None)
    }

    async fn insert_key_async(
        &self,
        key_id: &str,
        secret: &[u8; 32],
        public: &[u8; 32],
        created_at: OffsetDateTime,
        is_current: bool,
    ) -> Result<(), IdentityError> {
        let session = self.db_shell.create_session();
        let secret_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret);
        let public_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public);
        let created_str = created_at
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| IdentityError::Invalid(e.to_string()))?;

        let query = format!(
            "INSERT INTO identity_keys (key_id, environment, instance_id, secret_key, public_key, created_at, is_current) \
             VALUES ('{}', '{}', '{}', '{}', '{}', '{}', {})",
            escape_sql(key_id),
            escape_sql(&self.environment),
            escape_sql(&self.instance_id),
            escape_sql(&secret_b64),
            escape_sql(&public_b64),
            escape_sql(&created_str),
            is_current
        );

        session
            .simple_query(&query)
            .await
            .map_err(db_to_identity_error)?;
        Ok(())
    }

    async fn load_all_users_async(
        &self,
    ) -> Result<std::collections::BTreeMap<String, IdentityUserRecord>, IdentityError> {
        let session = self.db_shell.create_session();
        let query = "SELECT user_id, display_name, role, password_hash, password_updated_at, \
                     created_at, last_issued_at, token_count, last_token_fingerprint, last_login_at \
                     FROM identity_users";

        let results = session
            .simple_query(query)
            .await
            .map_err(db_to_identity_error)?;
        let mut users = std::collections::BTreeMap::new();

        if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
            for row in &rs.rows {
                let record = self.parse_user_row(row)?;
                users.insert(record.user_id.clone(), record);
            }
        }

        // Load tokens for each user
        for (user_id, user) in users.iter_mut() {
            let tokens = self.load_user_tokens_async(user_id).await?;
            user.tokens = tokens;
        }

        Ok(users)
    }

    async fn load_user_tokens_async(
        &self,
        user_id: &str,
    ) -> Result<Vec<IdentityTokenRecord>, IdentityError> {
        let session = self.db_shell.create_session();
        let query = format!(
            "SELECT token_id, fingerprint, issued_at, expires_at, key_id \
             FROM identity_tokens WHERE user_id = '{}'",
            escape_sql(user_id)
        );

        let results = session
            .simple_query(&query)
            .await
            .map_err(db_to_identity_error)?;
        let mut tokens = Vec::new();

        if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
            for row in &rs.rows {
                tokens.push(self.parse_token_row(row)?);
            }
        }

        Ok(tokens)
    }

    async fn is_password_set_async(&self, user_id: &str) -> Result<bool, IdentityError> {
        let session = self.db_shell.create_session();
        let query = format!(
            "SELECT password_hash FROM identity_users WHERE user_id = '{}'",
            escape_sql(user_id)
        );

        let results = session
            .simple_query(&query)
            .await
            .map_err(db_to_identity_error)?;

        if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
            if let Some(row) = rs.rows.first() {
                if let Some(hash) = row.first() {
                    return Ok(!hash.is_empty());
                }
            }
        }
        Ok(false)
    }

    async fn get_user_async(
        &self,
        user_id: &str,
    ) -> Result<Option<IdentityUserRecord>, IdentityError> {
        let session = self.db_shell.create_session();
        let query = format!(
            "SELECT user_id, display_name, role, password_hash, password_updated_at, \
             created_at, last_issued_at, token_count, last_token_fingerprint, last_login_at \
             FROM identity_users WHERE user_id = '{}'",
            escape_sql(user_id)
        );

        let results = session
            .simple_query(&query)
            .await
            .map_err(db_to_identity_error)?;

        if let Some(DbExecutionResult::ResultSet(rs)) = results.first() {
            if let Some(row) = rs.rows.first() {
                let mut record = self.parse_user_row(row)?;
                record.tokens = self.load_user_tokens_async(user_id).await?;
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    async fn set_password_async(
        &self,
        user_id: &str,
        password_hash: String,
        role: Role,
    ) -> Result<(), IdentityError> {
        let session = self.db_shell.create_session();
        let now = OffsetDateTime::now_utc();
        let now_str = now
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| IdentityError::Invalid(e.to_string()))?;

        // Check if user exists
        let existing = self.get_user_async(user_id).await?;

        if existing.is_some() {
            // Update existing user
            let query = format!(
                "UPDATE identity_users SET password_hash = '{}', password_updated_at = '{}' \
                 WHERE user_id = '{}'",
                escape_sql(&password_hash),
                escape_sql(&now_str),
                escape_sql(user_id)
            );
            session
                .simple_query(&query)
                .await
                .map_err(db_to_identity_error)?;
        } else {
            // Insert new user
            let query = format!(
                "INSERT INTO identity_users (user_id, display_name, role, password_hash, \
                 password_updated_at, created_at, token_count) \
                 VALUES ('{}', '{}', '{}', '{}', '{}', '{}', 0)",
                escape_sql(user_id),
                escape_sql(user_id),
                escape_sql(role.as_str()),
                escape_sql(&password_hash),
                escape_sql(&now_str),
                escape_sql(&now_str)
            );
            session
                .simple_query(&query)
                .await
                .map_err(db_to_identity_error)?;
        }

        // Write marker file so scripts can detect password is set without DB access
        if let Some(dir) = &self.pending_password_dir {
            let marker_path = dir.join(".db-password-set");
            if let Some(parent) = marker_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&marker_path, &now_str);
        }

        Ok(())
    }

    async fn record_login_async(&self, user_id: &str) -> Result<(), IdentityError> {
        let session = self.db_shell.create_session();
        let now = OffsetDateTime::now_utc();
        let now_str = now
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| IdentityError::Invalid(e.to_string()))?;

        let query = format!(
            "UPDATE identity_users SET last_login_at = '{}' WHERE user_id = '{}'",
            escape_sql(&now_str),
            escape_sql(user_id)
        );
        session
            .simple_query(&query)
            .await
            .map_err(db_to_identity_error)?;
        Ok(())
    }

    // === Row parsing helpers ===

    fn parse_key_row(&self, row: &[String]) -> Result<IdentityKeyMaterial, IdentityError> {
        let key_id = row.get(0).cloned().unwrap_or_default();
        let secret_b64 = row.get(1).cloned().unwrap_or_default();
        let public_b64 = row.get(2).cloned().unwrap_or_default();
        let created_str = row.get(3).cloned().unwrap_or_default();

        let secret = decode_key_bytes(&secret_b64)?;
        let public = decode_key_bytes(&public_b64)?;
        let created_at = parse_timestamp(&created_str)?;

        Ok(IdentityKeyMaterial::from_parts(
            key_id,
            secret,
            public,
            created_at,
        ))
    }

    fn parse_user_row(&self, row: &[String]) -> Result<IdentityUserRecord, IdentityError> {
        let user_id = row.get(0).cloned().unwrap_or_default();
        let display_name = row.get(1).cloned().filter(|s| !s.is_empty());
        let role_str = row.get(2).cloned().unwrap_or_default();
        let password_hash = row.get(3).cloned().filter(|s| !s.is_empty());
        let password_updated_at = row
            .get(4)
            .cloned()
            .filter(|s| !s.is_empty())
            .and_then(|s| parse_timestamp(&s).ok());
        let created_at = parse_timestamp(&row.get(5).cloned().unwrap_or_default())?;
        let last_issued_at = row
            .get(6)
            .cloned()
            .filter(|s| !s.is_empty())
            .and_then(|s| parse_timestamp(&s).ok());
        let token_count = row
            .get(7)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let last_token_fingerprint = row.get(8).cloned().filter(|s| !s.is_empty());
        let last_login_at = row
            .get(9)
            .cloned()
            .filter(|s| !s.is_empty())
            .and_then(|s| parse_timestamp(&s).ok());

        let role = std::str::FromStr::from_str(&role_str)
            .map_err(|_| IdentityError::Invalid(format!("Unknown role: {}", role_str)))?;

        Ok(IdentityUserRecord {
            user_id,
            display_name,
            role,
            created_at,
            last_issued_at,
            token_count,
            last_token_fingerprint,
            tokens: Vec::new(), // Loaded separately
            password_hash,
            password_updated_at,
            last_login_at,
        })
    }

    fn parse_token_row(&self, row: &[String]) -> Result<IdentityTokenRecord, IdentityError> {
        let token_id = row.get(0).cloned().unwrap_or_default();
        let fingerprint = row.get(1).cloned().unwrap_or_default();
        let issued_at = parse_timestamp(&row.get(2).cloned().unwrap_or_default())?;
        let expires_at = parse_timestamp(&row.get(3).cloned().unwrap_or_default())?;
        let key_id = row.get(4).cloned().unwrap_or_default();

        Ok(IdentityTokenRecord {
            token_id,
            fingerprint,
            issued_at,
            expires_at,
            key_id,
        })
    }
}

// === Helper functions ===

fn block_on<F: std::future::Future<Output = T>, T>(future: F) -> T {
    if let Ok(handle) = Handle::try_current() {
        task::block_in_place(|| handle.block_on(future))
    } else {
        // Create a new runtime if none exists
        tokio::runtime::Runtime::new()
            .expect("Failed to create runtime")
            .block_on(future)
    }
}

fn db_to_identity_error(err: DbError) -> IdentityError {
    IdentityError::Db(err.to_string())
}

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

fn decode_key_bytes(encoded: &str) -> Result<[u8; 32], IdentityError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|e| IdentityError::Invalid(format!("Invalid base64: {}", e)))?;

    if bytes.len() != 32 {
        return Err(IdentityError::Invalid(
            identity_messages::unexpected_key_length_in_store().into(),
        ));
    }

    let mut data = [0u8; 32];
    data.copy_from_slice(&bytes);
    Ok(data)
}

fn parse_timestamp(s: &str) -> Result<OffsetDateTime, IdentityError> {
    // Try RFC3339 first (e.g., "2025-12-18T10:23:42.957963+01:00")
    if let Ok(dt) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(dt);
    }

    // Try PostgreSQL default format (e.g., "2025-12-18 10:23:42.957963+01")
    // Format: YYYY-MM-DD HH:MM:SS.microseconds+TZ
    let pg_format = time::format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second][optional [.[subsecond]]][offset_hour]",
    )
    .ok();

    if let Some(fmt) = pg_format {
        if let Ok(dt) = OffsetDateTime::parse(s, &fmt) {
            return Ok(dt);
        }
    }

    // Try PostgreSQL format with full offset (e.g., "2025-12-18 10:23:42.957963+01:00")
    let pg_format_full = time::format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second][optional [.[subsecond]]][offset_hour]:[offset_minute]",
    )
    .ok();

    if let Some(fmt) = pg_format_full {
        if let Ok(dt) = OffsetDateTime::parse(s, &fmt) {
            return Ok(dt);
        }
    }

    // Fallback: try to convert PostgreSQL format to RFC3339 and parse
    // Replace space with 'T' and ensure offset has colon
    let normalized = s
        .replace(' ', "T")
        .chars()
        .collect::<String>();

    // Fix offset format if needed (e.g., "+01" -> "+01:00")
    let normalized = if normalized.ends_with("+00") || normalized.ends_with("-00") {
        format!("{}:00", normalized)
    } else if normalized.len() > 3 {
        let last_three: String = normalized.chars().rev().take(3).collect::<String>().chars().rev().collect();
        if (last_three.starts_with('+') || last_three.starts_with('-'))
            && last_three[1..].chars().all(|c| c.is_ascii_digit())
        {
            format!("{}:00", normalized)
        } else {
            normalized
        }
    } else {
        normalized
    };

    OffsetDateTime::parse(&normalized, &time::format_description::well_known::Rfc3339)
        .map_err(|e| IdentityError::Invalid(format!("Invalid timestamp '{}': {}", s, e)))
}
