-- Migration: Create Athene Password Reset Tokens Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_password_reset_tokens (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    user_id TEXT NOT NULL REFERENCES athene_users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    used_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_athene_password_reset_tokens_user_id ON athene_password_reset_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_athene_password_reset_tokens_token_hash ON athene_password_reset_tokens(token_hash);
