-- Migration: Create Athene Login Challenges Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_login_challenges (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    email TEXT NOT NULL,
    user_id TEXT REFERENCES athene_users(id) ON DELETE SET NULL,
    purpose TEXT NOT NULL CHECK (purpose IN ('login', 'register', 'password_reset')),
    pin_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    ip_address TEXT,
    user_agent TEXT
);

CREATE INDEX IF NOT EXISTS idx_athene_login_challenges_email ON athene_login_challenges(email);
CREATE INDEX IF NOT EXISTS idx_athene_login_challenges_user_id ON athene_login_challenges(user_id);
CREATE INDEX IF NOT EXISTS idx_athene_login_challenges_expires_at ON athene_login_challenges(expires_at);
