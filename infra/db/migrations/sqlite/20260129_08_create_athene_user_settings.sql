-- Migration: Create Athene User Settings Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athene_user_settings (
    user_id TEXT PRIMARY KEY REFERENCES athene_users(id) ON DELETE CASCADE,
    theme TEXT NOT NULL DEFAULT 'system' CHECK (theme IN ('light', 'dark', 'system')),
    language TEXT NOT NULL DEFAULT 'de',
    shortcuts TEXT NOT NULL DEFAULT '{}',
    notifications_enabled INTEGER NOT NULL DEFAULT 1,
    email_notifications INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
