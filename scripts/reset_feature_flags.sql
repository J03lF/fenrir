-- Reset Feature Flags to meaningful defaults
-- Run this in db-shell to clean up placeholder flags

-- Remove old placeholder flags that have no backend logic
DELETE FROM athene_feature_flags WHERE key IN (
    'registration_enabled',
    'beta_features',
    'public_projects',
    'team_invites',
    'api_access'
);

-- Insert meaningful flags (skip if already exist)
INSERT INTO athene_feature_flags (key, description, enabled, requires_api_key)
VALUES 
    ('registration', 'Allow new users to create accounts via self-registration', TRUE, FALSE),
    ('password_reset', 'Allow users to reset their password via email', TRUE, FALSE),
    ('api_keys', 'Enable API key creation and management for programmatic access', TRUE, FALSE),
    ('audit_log', 'Enable the audit log for tracking administrative actions', FALSE, FALSE)
ON CONFLICT (key) DO NOTHING;
