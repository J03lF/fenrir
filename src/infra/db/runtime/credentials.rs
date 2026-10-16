use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::security::crypto::CipherAlgorithm;
use crate::security::manager::SecurityManager;

const ENV_SEAL_KEY: &str = "FENRIR_DB_RUNTIME_KEY";
const DEFAULT_USER: &str = "fenrir_embedded";
const SEAL_KEY_FILE: &str = "db-seal.key";

/// Cached seal key (loaded once per process)
static CACHED_SEAL_KEY: OnceLock<String> = OnceLock::new();

#[derive(Debug, Serialize, Deserialize)]
pub struct SealedDbCredentials {
    pub user: String,
    pub cipher: String,
    pub nonce_b64: String,
    pub salt_b64: String,
    pub ciphertext_b64: String,
}

impl SealedDbCredentials {
    /// Ensure credentials exist on disk; create and seal if missing.
    pub async fn ensure(security: &SecurityManager, runtime_dir: &Path) -> anyhow::Result<()> {
        let path = sealed_credentials_path(runtime_dir);
        if path.exists() {
            // Attempt to read/decrypt once to verify.
            let _ = Self::load(security, runtime_dir).await?;
            return Ok(());
        }
        let creds = Self::generate_and_seal(security, runtime_dir).await?;
        write_json(&path, &creds).await?;
        Ok(())
    }

    pub async fn load(
        security: &SecurityManager,
        runtime_dir: &Path,
    ) -> anyhow::Result<DbCredentials> {
        let path = sealed_credentials_path(runtime_dir);
        let bytes = fs::read(&path).await?;
        let sealed: SealedDbCredentials = serde_json::from_slice(&bytes)?;
        sealed.decrypt(security, runtime_dir)
    }

    /// Load if exists, otherwise generate, seal, and return freshly created credentials.
    pub async fn load_or_create(
        security: &SecurityManager,
        runtime_dir: &Path,
    ) -> anyhow::Result<DbCredentials> {
        let path = sealed_credentials_path(runtime_dir);
        if path.exists() {
            return Self::load(security, runtime_dir).await;
        }
        let creds = DbCredentials {
            user: DEFAULT_USER.to_string(),
            password: random_password(32),
        };
        let sealed = Self::encrypt(security, runtime_dir, creds.clone())?;
        write_json(&path, &sealed).await?;
        Ok(creds)
    }

    async fn generate_and_seal(
        security: &SecurityManager,
        runtime_dir: &Path,
    ) -> anyhow::Result<Self> {
        let password = random_password(32);
        let creds = DbCredentials {
            user: DEFAULT_USER.to_string(),
            password,
        };
        Self::encrypt(security, runtime_dir, creds)
    }

    fn encrypt(
        security: &SecurityManager,
        runtime_dir: &Path,
        creds: DbCredentials,
    ) -> anyhow::Result<SealedDbCredentials> {
        let cipher = CipherAlgorithm::XChaCha20Poly1305;
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let key = derive_key(security, runtime_dir, &salt)?;
        let nonce = security.generate_nonce(cipher)?;
        let plaintext = serde_json::to_vec(&creds)?;
        let ciphertext =
            security.encrypt(cipher, &key, &nonce, &plaintext, b"fenrir-db-runtime")?;
        Ok(SealedDbCredentials {
            user: creds.user,
            cipher: cipher.to_string(),
            nonce_b64: STANDARD.encode(nonce),
            salt_b64: STANDARD.encode(salt),
            ciphertext_b64: STANDARD.encode(ciphertext),
        })
    }

    fn decrypt(
        self,
        security: &SecurityManager,
        runtime_dir: &Path,
    ) -> anyhow::Result<DbCredentials> {
        let cipher = CipherAlgorithm::try_from(self.cipher.as_str())
            .map_err(|e| anyhow::anyhow!("invalid cipher: {e}"))?;
        let nonce = STANDARD
            .decode(self.nonce_b64.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid nonce b64: {e}"))?;
        let salt = STANDARD
            .decode(self.salt_b64.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid salt b64: {e}"))?;
        let key = derive_key(security, runtime_dir, &salt)?;
        let ciphertext = STANDARD
            .decode(self.ciphertext_b64.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid ciphertext b64: {e}"))?;
        let plaintext =
            security.decrypt(cipher, &key, &nonce, &ciphertext, b"fenrir-db-runtime")?;
        let creds: DbCredentials = serde_json::from_slice(&plaintext)?;
        Ok(creds)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DbCredentials {
    pub user: String,
    pub password: String,
}

/// Get the seal key from ENV or auto-generated file.
/// Priority: ENV > file > auto-generate
fn get_seal_key(runtime_dir: &Path) -> anyhow::Result<String> {
    // Return cached key if available
    if let Some(key) = CACHED_SEAL_KEY.get() {
        return Ok(key.clone());
    }

    // 1. Try ENV first (for production / CI)
    if let Ok(key) = std::env::var(ENV_SEAL_KEY) {
        if !key.trim().is_empty() {
            let _ = CACHED_SEAL_KEY.set(key.clone());
            return Ok(key);
        }
    }

    // 2. Try loading from file
    let key_path = seal_key_path(runtime_dir);
    if key_path.exists() {
        let key = std::fs::read_to_string(&key_path)
            .map_err(|e| anyhow::anyhow!("failed to read seal key: {e}"))?;
        let key = key.trim().to_string();
        if !key.is_empty() {
            let _ = CACHED_SEAL_KEY.set(key.clone());
            return Ok(key);
        }
    }

    // 3. Auto-generate and save
    let key = generate_seal_key();

    // Ensure directory exists
    if let Some(parent) = key_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write key file with restricted permissions
    std::fs::write(&key_path, &key)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&key_path, perms)?;
    }

    info!(path = %key_path.display(), "auto-generated db seal key");
    let _ = CACHED_SEAL_KEY.set(key.clone());
    Ok(key)
}

fn generate_seal_key() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}

fn seal_key_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(SEAL_KEY_FILE)
}

fn derive_key(
    security: &SecurityManager,
    runtime_dir: &Path,
    salt: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let secret = get_seal_key(runtime_dir)?;
    security
        .derive_key(secret.as_bytes(), salt)
        .map_err(|e| anyhow::anyhow!(e))
}

fn random_password(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    STANDARD.encode(bytes)
}

fn sealed_credentials_path(runtime_dir: &Path) -> std::path::PathBuf {
    runtime_dir.join("internal-secrets.json")
}

async fn write_json(path: &Path, value: &SealedDbCredentials) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let serialized = serde_json::to_vec_pretty(value)?;
    let mut file = fs::File::create(path).await?;
    file.write_all(&serialized).await?;
    Ok(())
}
