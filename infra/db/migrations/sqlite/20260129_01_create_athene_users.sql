-- Migration: Create Athene Users Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_users (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    email TEXT NOT NULL UNIQUE,
    email_verified INTEGER NOT NULL DEFAULT 0,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'operator', 'user', 'guest')),
    status TEXT NOT NULL DEFAULT 'pending_verification' CHECK (status IN ('active', 'inactive', 'locked', 'pending_verification')),
    failed_login_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_login_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_athene_users_email ON athene_users(email);
CREATE INDEX IF NOT EXISTS idx_athene_users_status ON athene_users(status);
CREATE INDEX IF NOT EXISTS idx_athene_users_role ON athene_users(role);

-- Trigger for updated_at
CREATE TRIGGER IF NOT EXISTS trg_athene_users_updated_at
    AFTER UPDATE ON athene_users
    FOR EACH ROW
BEGIN
    UPDATE athene_users SET updated_at = datetime('now') WHERE id = NEW.id;
END;
