-- Migration: Create Athene App Settings Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_app_settings (
    id INTEGER PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    registration_mode TEXT NOT NULL DEFAULT 'disabled' CHECK (registration_mode IN ('disabled', 'api_key_required', 'open')),
    maintenance_mode INTEGER NOT NULL DEFAULT 0,
    maintenance_message TEXT,
    allowed_email_domains TEXT NOT NULL DEFAULT '[]',
    max_users INTEGER NOT NULL DEFAULT 0,
    max_teams INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_by TEXT REFERENCES athene_users(id)
);

-- Insert default settings
INSERT OR IGNORE INTO athene_app_settings (id) VALUES (1);
