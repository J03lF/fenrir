-- Migration: Create Athene Setup Token Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_setup_tokens (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    used_at TEXT,
    used_by TEXT REFERENCES athene_users(id)
);

CREATE INDEX IF NOT EXISTS idx_athene_setup_tokens_token_hash ON athene_setup_tokens(token_hash);
