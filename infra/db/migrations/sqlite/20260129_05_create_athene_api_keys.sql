-- Migration: Create Athene API Keys Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_api_keys (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    user_id TEXT REFERENCES athene_users(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked', 'expired')),
    scopes TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    last_used_at TEXT,
    created_by TEXT NOT NULL REFERENCES athene_users(id)
);

CREATE INDEX IF NOT EXISTS idx_athene_api_keys_key_hash ON athene_api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_status ON athene_api_keys(status);
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_user_id ON athene_api_keys(user_id);
