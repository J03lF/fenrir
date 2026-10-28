-- Migration: Create Athenos Sessions Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athenos_sessions (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    user_id TEXT NOT NULL REFERENCES athenos_users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'expired', 'revoked')),
    ip_address TEXT,
    user_agent TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    last_activity TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_athenos_sessions_user_id ON athenos_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_token_hash ON athenos_sessions(token_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_status ON athenos_sessions(status);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_expires_at ON athenos_sessions(expires_at);
