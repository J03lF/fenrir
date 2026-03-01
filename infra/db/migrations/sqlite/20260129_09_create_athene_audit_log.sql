-- Migration: Create Athene Audit Log Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_audit_log (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    actor_id TEXT REFERENCES athene_users(id) ON DELETE SET NULL,
    actor_email TEXT,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    details TEXT,
    ip_address TEXT,
    user_agent TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_athene_audit_log_actor_id ON athene_audit_log(actor_id);
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_action ON athene_audit_log(action);
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_created_at ON athene_audit_log(created_at);
