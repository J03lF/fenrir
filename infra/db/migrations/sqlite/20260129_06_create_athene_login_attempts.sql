-- Migration: Create Athene Login Attempts Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_login_attempts (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    email TEXT NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    success INTEGER NOT NULL,
    failure_reason TEXT,
    attempted_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_email ON athene_login_attempts(email);
CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_attempted_at ON athene_login_attempts(attempted_at);
