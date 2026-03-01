-- Migration: Create Athene Feature Flags Table
-- Version: 20260129
-- Description: Feature flags for controlling application features

CREATE TABLE IF NOT EXISTS athene_feature_flags (
    key VARCHAR(100) PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    requires_api_key BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_by UUID REFERENCES athene_users(id)
);

-- Index
CREATE INDEX IF NOT EXISTS idx_athene_feature_flags_enabled ON athene_feature_flags(enabled);

-- Trigger for updated_at
CREATE OR REPLACE FUNCTION update_athene_feature_flags_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_athene_feature_flags_updated_at ON athene_feature_flags;
CREATE TRIGGER trg_athene_feature_flags_updated_at
    BEFORE UPDATE ON athene_feature_flags
    FOR EACH ROW
    EXECUTE FUNCTION update_athene_feature_flags_updated_at();

-- Insert default feature flags
-- These flags are checked at runtime by the frontend and backend:
--   registration     → controls self-registration availability
--   password_reset   → controls password reset flow availability
--   api_keys         → controls API key management visibility
--   audit_log        → controls audit log access
INSERT INTO athene_feature_flags (key, description, enabled, requires_api_key)
VALUES 
    ('registration', 'Allow new users to create accounts via self-registration', TRUE, FALSE),
    ('password_reset', 'Allow users to reset their password via email', TRUE, FALSE),
    ('api_keys', 'Enable API key creation and management for programmatic access', TRUE, FALSE),
    ('audit_log', 'Enable the audit log for tracking administrative actions', FALSE, FALSE)
ON CONFLICT (key) DO NOTHING;
