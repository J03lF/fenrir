-- Migration: Create Athenos User Settings Table
-- Version: 20260129
-- Description: Per-user settings including keyboard shortcuts

CREATE TABLE IF NOT EXISTS athenos_user_settings (
    user_id UUID PRIMARY KEY REFERENCES athenos_users(id) ON DELETE CASCADE,
    theme VARCHAR(20) NOT NULL DEFAULT 'system',
    language VARCHAR(10) NOT NULL DEFAULT 'de',
    shortcuts JSONB NOT NULL DEFAULT '{}',
    notifications_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    email_notifications BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    CONSTRAINT chk_theme CHECK (theme IN ('light', 'dark', 'system'))
);

-- Trigger for updated_at
CREATE OR REPLACE FUNCTION update_athenos_user_settings_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_athenos_user_settings_updated_at ON athenos_user_settings;
CREATE TRIGGER trg_athenos_user_settings_updated_at
    BEFORE UPDATE ON athenos_user_settings
    FOR EACH ROW
    EXECUTE FUNCTION update_athenos_user_settings_updated_at();
