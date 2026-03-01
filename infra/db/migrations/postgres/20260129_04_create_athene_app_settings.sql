-- Migration: Create Athene App Settings Table
-- Version: 20260129
-- Description: Global application settings

CREATE TABLE IF NOT EXISTS athene_app_settings (
    id INTEGER PRIMARY KEY DEFAULT 1,
    registration_mode VARCHAR(50) NOT NULL DEFAULT 'disabled',
    maintenance_mode BOOLEAN NOT NULL DEFAULT FALSE,
    maintenance_message TEXT,
    allowed_email_domains TEXT[] NOT NULL DEFAULT '{}',
    max_users INTEGER NOT NULL DEFAULT 0,
    max_teams INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by UUID REFERENCES athene_users(id),
    
    CONSTRAINT chk_registration_mode CHECK (registration_mode IN ('disabled', 'api_key_required', 'open')),
    CONSTRAINT single_row CHECK (id = 1)
);

-- Insert default settings
INSERT INTO athene_app_settings (id)
VALUES (1)
ON CONFLICT (id) DO NOTHING;

-- Trigger for updated_at
CREATE OR REPLACE FUNCTION update_athene_app_settings_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_athene_app_settings_updated_at ON athene_app_settings;
CREATE TRIGGER trg_athene_app_settings_updated_at
    BEFORE UPDATE ON athene_app_settings
    FOR EACH ROW
    EXECUTE FUNCTION update_athene_app_settings_updated_at();
