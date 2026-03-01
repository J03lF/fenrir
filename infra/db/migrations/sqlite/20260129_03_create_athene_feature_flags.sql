-- Migration: Create Athene Feature Flags Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_feature_flags (
    key TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 0,
    requires_api_key INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_by TEXT REFERENCES athene_users(id)
);

CREATE INDEX IF NOT EXISTS idx_athene_feature_flags_enabled ON athene_feature_flags(enabled);

-- Insert default feature flags
INSERT OR IGNORE INTO athene_feature_flags (key, description, enabled, requires_api_key)
VALUES 
    ('registration', 'Allow new users to create accounts via self-registration', 1, 0),
    ('password_reset', 'Allow users to reset their password via email', 1, 0),
    ('api_keys', 'Enable API key creation and management for programmatic access', 1, 0),
    ('audit_log', 'Enable the audit log for tracking administrative actions', 0, 0);
