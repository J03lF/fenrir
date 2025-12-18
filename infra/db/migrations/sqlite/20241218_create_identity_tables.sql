-- Identity Key Material (signing keys for JWT tokens)
CREATE TABLE IF NOT EXISTS identity_keys (
    key_id TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    secret_key TEXT NOT NULL,
    public_key TEXT NOT NULL,
    created_at TEXT NOT NULL,
    is_current INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_identity_keys_current
    ON identity_keys (is_current);

-- Identity Users
CREATE TABLE IF NOT EXISTS identity_users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT,
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'viewer')),
    password_hash TEXT,
    password_updated_at TEXT,
    created_at TEXT NOT NULL,
    last_issued_at TEXT,
    token_count INTEGER NOT NULL DEFAULT 0,
    last_token_fingerprint TEXT,
    last_login_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_identity_users_role
    ON identity_users (role);

-- Identity Tokens (issued JWT tokens tracking)
CREATE TABLE IF NOT EXISTS identity_tokens (
    token_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES identity_users(user_id) ON DELETE CASCADE,
    key_id TEXT NOT NULL REFERENCES identity_keys(key_id) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_identity_tokens_user_id
    ON identity_tokens (user_id);

CREATE INDEX IF NOT EXISTS idx_identity_tokens_expires_at
    ON identity_tokens (expires_at);

