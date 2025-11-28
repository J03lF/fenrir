use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::RwLock;

use base64::Engine;
use ed25519_dalek::{Keypair, PublicKey, SecretKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use super::IdentityError;
use crate::security::auth::Role;
use crate::utils::messages::security::identity as identity_messages;

const STORE_VERSION: u32 = 1;

#[derive(Clone)]
pub struct IdentityKeyMaterial {
    pub key_id: String,
    pub created_at: OffsetDateTime,
    secret_key: [u8; 32],
    public_key: [u8; 32],
}

impl IdentityKeyMaterial {
    fn new(
        key_id: String,
        secret_key: [u8; 32],
        public_key: [u8; 32],
        created_at: OffsetDateTime,
    ) -> Self {
        Self {
            key_id,
            created_at,
            secret_key,
            public_key,
        }
    }

    pub fn keypair(&self) -> Result<Keypair, IdentityError> {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&self.secret_key);
        bytes[32..].copy_from_slice(&self.public_key);
        Keypair::from_bytes(&bytes).map_err(|_| {
            IdentityError::Invalid(identity_messages::stored_key_material_invalid().into())
        })
    }

    pub fn verifying_key(&self) -> Result<PublicKey, IdentityError> {
        PublicKey::from_bytes(&self.public_key).map_err(|_| {
            IdentityError::Invalid(identity_messages::stored_public_key_invalid().into())
        })
    }

    pub fn secret_bytes(&self) -> &[u8; 32] {
        &self.secret_key
    }

    pub fn public_bytes(&self) -> &[u8; 32] {
        &self.public_key
    }
}

#[derive(Clone)]
pub struct IdentityUserRecord {
    pub user_id: String,
    pub display_name: Option<String>,
    pub role: Role,
    pub created_at: OffsetDateTime,
    pub last_issued_at: Option<OffsetDateTime>,
    pub token_count: u64,
    pub last_token_fingerprint: Option<String>,
    pub tokens: Vec<IdentityTokenRecord>,
    pub password_hash: Option<String>,
    pub password_updated_at: Option<OffsetDateTime>,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Clone)]
pub struct IdentityState {
    pub environment: String,
    pub instance_id: String,
    pub app_version: String,
    pub current_key: IdentityKeyMaterial,
    pub users: BTreeMap<String, IdentityUserRecord>,
}

pub struct IdentityStore {
    path: PathBuf,
    state: RwLock<IdentityState>,
}

impl IdentityStore {
    pub fn load_or_initialize(
        path: PathBuf,
        environment: String,
        instance_id: String,
        app_version: String,
    ) -> Result<Self, IdentityError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let state = if path.exists() {
            let bytes = fs::read(&path)?;
            let persisted: PersistedIdentityState = serde_json::from_slice(&bytes)?;
            if persisted.version != STORE_VERSION {
                return Err(IdentityError::Invalid(
                    identity_messages::unsupported_identity_store_version(persisted.version),
                ));
            }
            if persisted.environment != environment {
                return Err(IdentityError::Invalid(
                    identity_messages::identity_store_environment_mismatch(
                        &environment,
                        &persisted.environment,
                    ),
                ));
            }
            if persisted.instance_id != instance_id {
                return Err(IdentityError::Invalid(
                    identity_messages::identity_store_instance_mismatch(
                        &instance_id,
                        &persisted.instance_id,
                    ),
                ));
            }

            IdentityState {
                environment,
                instance_id,
                app_version,
                current_key: persisted.current_key.try_into()?,
                users: persisted
                    .users
                    .into_iter()
                    .map(|user| {
                        let record: IdentityUserRecord = user.try_into()?;
                        Ok((record.user_id.clone(), record))
                    })
                    .collect::<Result<_, IdentityError>>()?,
            }
        } else {
            let mut secret_bytes = [0u8; 32];
            OsRng.fill_bytes(&mut secret_bytes);
            let secret = SecretKey::from_bytes(&secret_bytes).map_err(|_| {
                IdentityError::Invalid(identity_messages::unable_to_initialize_secret_key().into())
            })?;
            let public: PublicKey = (&secret).into();
            let created_at = OffsetDateTime::now_utc();
            IdentityState {
                environment,
                instance_id,
                app_version,
                current_key: IdentityKeyMaterial::new(
                    format!("kid-{}", Uuid::new_v4()),
                    secret.to_bytes(),
                    public.to_bytes(),
                    created_at,
                ),
                users: BTreeMap::new(),
            }
        };

        let store = Self {
            path,
            state: RwLock::new(state),
        };
        store.persist()?;
        Ok(store)
    }

    pub fn read<F, R>(&self, reader: F) -> Result<R, IdentityError>
    where
        F: FnOnce(&IdentityState) -> R,
    {
        let guard = self.state.read()?;
        Ok(reader(&guard))
    }

    pub fn write<F, R>(&self, writer: F) -> Result<R, IdentityError>
    where
        F: FnOnce(&mut IdentityState) -> Result<R, IdentityError>,
    {
        let mut guard = self.state.write()?;
        let result = writer(&mut guard)?;
        self.persist_state(&guard)?;
        Ok(result)
    }

    pub fn persist(&self) -> Result<(), IdentityError> {
        let guard = self.state.read()?;
        self.persist_state(&guard)
    }

    fn persist_state(&self, state: &IdentityState) -> Result<(), IdentityError> {
        let persisted = PersistedIdentityState::from(state.clone());
        let json = serde_json::to_vec_pretty(&persisted)?;
        let tmp_path = temp_path(&self.path);
        if let Some(parent) = tmp_path.parent() {
            fs::create_dir_all(parent)?;
        }
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(&json)?;
            file.sync_all()?;
        }
        fs::rename(tmp_path, &self.path)?;
        Ok(())
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let filename = match path.file_name() {
        Some(name) => format!("{}.tmp", name.to_string_lossy()),
        None => "identity.tmp".to_string(),
    };
    tmp.set_file_name(filename);
    tmp
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentityState {
    version: u32,
    environment: String,
    instance_id: String,
    app_version: String,
    current_key: PersistedIdentityKey,
    users: Vec<PersistedIdentityUser>,
}

impl From<IdentityState> for PersistedIdentityState {
    fn from(state: IdentityState) -> Self {
        Self {
            version: STORE_VERSION,
            environment: state.environment,
            instance_id: state.instance_id,
            app_version: state.app_version,
            current_key: state.current_key.into(),
            users: state.users.into_values().map(|user| user.into()).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentityKey {
    key_id: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    secret_key: String,
    public_key: String,
}

impl From<IdentityKeyMaterial> for PersistedIdentityKey {
    fn from(material: IdentityKeyMaterial) -> Self {
        let secret = *material.secret_bytes();
        let public = *material.public_bytes();
        let key_id = material.key_id;
        let created_at = material.created_at;
        Self {
            key_id,
            created_at,
            secret_key: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
            public_key: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public),
        }
    }
}

impl TryFrom<PersistedIdentityKey> for IdentityKeyMaterial {
    type Error = IdentityError;

    fn try_from(value: PersistedIdentityKey) -> Result<Self, Self::Error> {
        let secret = decode_key_component(&value.secret_key)?;
        let public = decode_key_component(&value.public_key)?;
        Ok(IdentityKeyMaterial::new(
            value.key_id,
            secret,
            public,
            value.created_at,
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentityUser {
    user_id: String,
    display_name: Option<String>,
    role: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "option_rfc3339")]
    last_issued_at: Option<OffsetDateTime>,
    token_count: u64,
    last_token_fingerprint: Option<String>,
    #[serde(default)]
    tokens: Vec<PersistedIdentityToken>,
    #[serde(default)]
    password_hash: Option<String>,
    #[serde(default, with = "option_rfc3339")]
    password_updated_at: Option<OffsetDateTime>,
    #[serde(default, with = "option_rfc3339")]
    last_login_at: Option<OffsetDateTime>,
}

impl From<IdentityUserRecord> for PersistedIdentityUser {
    fn from(record: IdentityUserRecord) -> Self {
        Self {
            user_id: record.user_id,
            display_name: record.display_name,
            role: record.role.as_str().to_string(),
            created_at: record.created_at,
            last_issued_at: record.last_issued_at,
            token_count: record.token_count,
            last_token_fingerprint: record.last_token_fingerprint,
            tokens: record
                .tokens
                .into_iter()
                .map(PersistedIdentityToken::from)
                .collect(),
            password_hash: record.password_hash,
            password_updated_at: record.password_updated_at,
            last_login_at: record.last_login_at,
        }
    }
}

impl TryFrom<PersistedIdentityUser> for IdentityUserRecord {
    type Error = IdentityError;

    fn try_from(value: PersistedIdentityUser) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: value.user_id,
            display_name: value.display_name,
            role: Role::from_str(&value.role).map_err(|err| {
                IdentityError::Invalid(identity_messages::unknown_role_in_identity_store(
                    err.value(),
                ))
            })?,
            created_at: value.created_at,
            last_issued_at: value.last_issued_at,
            token_count: value.token_count,
            last_token_fingerprint: value.last_token_fingerprint,
            tokens: value
                .tokens
                .into_iter()
                .map(IdentityTokenRecord::try_from)
                .collect::<Result<_, _>>()?,
            password_hash: value.password_hash,
            password_updated_at: value.password_updated_at,
            last_login_at: value.last_login_at,
        })
    }
}

#[derive(Clone)]
pub struct IdentityTokenRecord {
    pub token_id: String,
    pub fingerprint: String,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub key_id: String,
}

#[derive(Serialize, Deserialize)]
struct PersistedIdentityToken {
    token_id: String,
    fingerprint: String,
    #[serde(with = "time::serde::rfc3339")]
    issued_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
    key_id: String,
}

impl From<IdentityTokenRecord> for PersistedIdentityToken {
    fn from(record: IdentityTokenRecord) -> Self {
        Self {
            token_id: record.token_id,
            fingerprint: record.fingerprint,
            issued_at: record.issued_at,
            expires_at: record.expires_at,
            key_id: record.key_id,
        }
    }
}

impl TryFrom<PersistedIdentityToken> for IdentityTokenRecord {
    type Error = IdentityError;

    fn try_from(value: PersistedIdentityToken) -> Result<Self, Self::Error> {
        Ok(Self {
            token_id: value.token_id,
            fingerprint: value.fingerprint,
            issued_at: value.issued_at,
            expires_at: value.expires_at,
            key_id: value.key_id,
        })
    }
}

fn decode_key_component(encoded: &str) -> Result<[u8; 32], IdentityError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|err| {
            IdentityError::Invalid(identity_messages::invalid_base64(&err.to_string()))
        })?;
    if bytes.len() != 32 {
        return Err(IdentityError::Invalid(
            identity_messages::unexpected_key_length_in_store().into(),
        ));
    }
    let mut data = [0u8; 32];
    data.copy_from_slice(&bytes);
    Ok(data)
}

mod option_rfc3339 {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::OffsetDateTime;

    pub fn serialize<S>(value: &Option<OffsetDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(dt) => serializer.serialize_some(
                &dt.format(&time::format_description::well_known::Rfc3339)
                    .map_err(serde::ser::Error::custom)?,
            ),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(value) => {
                OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
            None => Ok(None),
        }
    }
}

impl From<std::io::Error> for IdentityError {
    fn from(err: std::io::Error) -> Self {
        IdentityError::Io(err)
    }
}

impl From<serde_json::Error> for IdentityError {
    fn from(err: serde_json::Error) -> Self {
        IdentityError::Serde(err)
    }
}

impl<T> From<std::sync::PoisonError<T>> for IdentityError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        IdentityError::StatePoisoned
    }
}
