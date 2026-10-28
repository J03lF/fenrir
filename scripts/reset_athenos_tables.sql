DROP TABLE IF EXISTS athenos_login_challenges CASCADE; --force
DROP TABLE IF EXISTS athenos_login_attempts CASCADE; --force
DROP TABLE IF EXISTS athenos_password_reset_tokens CASCADE; --force
DROP TABLE IF EXISTS athenos_user_settings CASCADE; --force
DROP TABLE IF EXISTS athenos_audit_log CASCADE; --force
DROP TABLE IF EXISTS athenos_email_queue CASCADE; --force
DROP TABLE IF EXISTS athenos_setup_token CASCADE; --force
DROP TABLE IF EXISTS athenos_api_keys CASCADE; --force
DROP TABLE IF EXISTS athenos_sessions CASCADE; --force
DROP TABLE IF EXISTS athenos_feature_flags CASCADE; --force
DROP TABLE IF EXISTS athenos_app_settings CASCADE; --force
DROP TABLE IF EXISTS athenos_users CASCADE; --force
DROP FUNCTION IF EXISTS update_athenos_users_updated_at() CASCADE; --force
DROP FUNCTION IF EXISTS update_athenos_feature_flags_updated_at() CASCADE; --force
DROP FUNCTION IF EXISTS update_athenos_app_settings_updated_at() CASCADE; --force

CREATE TABLE IF NOT EXISTS athenos_users (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), email VARCHAR(255) NOT NULL UNIQUE, email_verified BOOLEAN NOT NULL DEFAULT FALSE, display_name VARCHAR(255) NOT NULL, password_hash VARCHAR(255) NOT NULL, role VARCHAR(50) NOT NULL DEFAULT 'user', status VARCHAR(50) NOT NULL DEFAULT 'pending_verification', failed_login_attempts INTEGER NOT NULL DEFAULT 0, locked_until TIMESTAMPTZ, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), last_login_at TIMESTAMPTZ, CONSTRAINT chk_role CHECK (role IN ('admin', 'operator', 'user', 'guest')), CONSTRAINT chk_status CHECK (status IN ('active', 'inactive', 'locked', 'pending_verification')));

CREATE INDEX IF NOT EXISTS idx_athenos_users_email ON athenos_users(email);
CREATE INDEX IF NOT EXISTS idx_athenos_users_status ON athenos_users(status);
CREATE INDEX IF NOT EXISTS idx_athenos_users_role ON athenos_users(role);

CREATE OR REPLACE FUNCTION update_athenos_users_updated_at() RETURNS TRIGGER AS $$ BEGIN NEW.updated_at = NOW(); RETURN NEW; END; $$ LANGUAGE plpgsql;

CREATE TRIGGER trg_athenos_users_updated_at BEFORE UPDATE ON athenos_users FOR EACH ROW EXECUTE FUNCTION update_athenos_users_updated_at();

CREATE TABLE IF NOT EXISTS athenos_sessions (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), user_id UUID NOT NULL REFERENCES athenos_users(id) ON DELETE CASCADE, token_hash VARCHAR(255) NOT NULL UNIQUE, status VARCHAR(50) NOT NULL DEFAULT 'active', ip_address INET, user_agent TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), expires_at TIMESTAMPTZ NOT NULL, last_activity TIMESTAMPTZ NOT NULL DEFAULT NOW(), CONSTRAINT chk_session_status CHECK (status IN ('active', 'expired', 'revoked')));

CREATE INDEX IF NOT EXISTS idx_athenos_sessions_user_id ON athenos_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_token_hash ON athenos_sessions(token_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_status ON athenos_sessions(status);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_expires_at ON athenos_sessions(expires_at);

CREATE TABLE IF NOT EXISTS athenos_feature_flags (key VARCHAR(100) PRIMARY KEY, description TEXT NOT NULL DEFAULT '', enabled BOOLEAN NOT NULL DEFAULT FALSE, requires_api_key BOOLEAN NOT NULL DEFAULT FALSE, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_by UUID REFERENCES athenos_users(id));

CREATE INDEX IF NOT EXISTS idx_athenos_feature_flags_enabled ON athenos_feature_flags(enabled);

CREATE OR REPLACE FUNCTION update_athenos_feature_flags_updated_at() RETURNS TRIGGER AS $$ BEGIN NEW.updated_at = NOW(); RETURN NEW; END; $$ LANGUAGE plpgsql;

CREATE TRIGGER trg_athenos_feature_flags_updated_at BEFORE UPDATE ON athenos_feature_flags FOR EACH ROW EXECUTE FUNCTION update_athenos_feature_flags_updated_at();

INSERT INTO athenos_feature_flags (key, description, enabled, requires_api_key) VALUES ('registration_enabled', 'Allow new user registration', FALSE, TRUE), ('beta_features', 'Enable beta features for testing', FALSE, TRUE), ('public_projects', 'Allow public project visibility', FALSE, FALSE), ('team_invites', 'Allow team invitations', FALSE, FALSE), ('api_access', 'Enable API access', TRUE, TRUE) ON CONFLICT (key) DO NOTHING;

CREATE TABLE IF NOT EXISTS athenos_app_settings (id INTEGER PRIMARY KEY DEFAULT 1, registration_mode VARCHAR(50) NOT NULL DEFAULT 'open', maintenance_mode BOOLEAN NOT NULL DEFAULT FALSE, maintenance_message TEXT, maintenance_until TIMESTAMPTZ, allowed_email_domains TEXT[] NOT NULL DEFAULT '{}', max_users INTEGER NOT NULL DEFAULT 0, max_teams INTEGER NOT NULL DEFAULT 0, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_by UUID REFERENCES athenos_users(id), CONSTRAINT chk_registration_mode CHECK (registration_mode IN ('disabled', 'api_key_required', 'open')), CONSTRAINT single_row CHECK (id = 1));

INSERT INTO athenos_app_settings (id, registration_mode) VALUES (1, 'open') ON CONFLICT (id) DO UPDATE SET registration_mode = 'open';

CREATE OR REPLACE FUNCTION update_athenos_app_settings_updated_at() RETURNS TRIGGER AS $$ BEGIN NEW.updated_at = NOW(); RETURN NEW; END; $$ LANGUAGE plpgsql;

CREATE TRIGGER trg_athenos_app_settings_updated_at BEFORE UPDATE ON athenos_app_settings FOR EACH ROW EXECUTE FUNCTION update_athenos_app_settings_updated_at();

CREATE TABLE IF NOT EXISTS athenos_api_keys (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), name VARCHAR(255) NOT NULL, key_hash VARCHAR(255) NOT NULL UNIQUE, key_prefix VARCHAR(20) NOT NULL, user_id UUID REFERENCES athenos_users(id) ON DELETE SET NULL, status VARCHAR(50) NOT NULL DEFAULT 'active', scopes TEXT[] NOT NULL DEFAULT '{}', created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), expires_at TIMESTAMPTZ, last_used_at TIMESTAMPTZ, created_by UUID NOT NULL REFERENCES athenos_users(id), CONSTRAINT chk_api_key_status CHECK (status IN ('active', 'revoked', 'expired')));

CREATE INDEX IF NOT EXISTS idx_athenos_api_keys_key_hash ON athenos_api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_api_keys_status ON athenos_api_keys(status);
CREATE INDEX IF NOT EXISTS idx_athenos_api_keys_user_id ON athenos_api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_api_keys_created_by ON athenos_api_keys(created_by);

CREATE TABLE IF NOT EXISTS athenos_login_attempts (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), email VARCHAR(255) NOT NULL, ip_address INET, user_agent TEXT, success BOOLEAN NOT NULL, failure_reason VARCHAR(255), attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW());

CREATE INDEX IF NOT EXISTS idx_athenos_login_attempts_email ON athenos_login_attempts(email);
CREATE INDEX IF NOT EXISTS idx_athenos_login_attempts_ip_address ON athenos_login_attempts(ip_address);
CREATE INDEX IF NOT EXISTS idx_athenos_login_attempts_attempted_at ON athenos_login_attempts(attempted_at);
CREATE INDEX IF NOT EXISTS idx_athenos_login_attempts_success ON athenos_login_attempts(success);

CREATE TABLE IF NOT EXISTS athenos_password_reset_tokens (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), user_id UUID NOT NULL REFERENCES athenos_users(id) ON DELETE CASCADE, token_hash VARCHAR(255) NOT NULL UNIQUE, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), expires_at TIMESTAMPTZ NOT NULL, used_at TIMESTAMPTZ);

CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_user_id ON athenos_password_reset_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_token_hash ON athenos_password_reset_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_expires_at ON athenos_password_reset_tokens(expires_at);

CREATE TABLE IF NOT EXISTS athenos_user_settings (user_id UUID PRIMARY KEY REFERENCES athenos_users(id) ON DELETE CASCADE, theme VARCHAR(50) NOT NULL DEFAULT 'system', language VARCHAR(10) NOT NULL DEFAULT 'en', timezone VARCHAR(100) NOT NULL DEFAULT 'UTC', notifications_enabled BOOLEAN NOT NULL DEFAULT TRUE, email_notifications BOOLEAN NOT NULL DEFAULT TRUE, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW());

CREATE TABLE IF NOT EXISTS athenos_audit_log (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), actor_id UUID REFERENCES athenos_users(id) ON DELETE SET NULL, actor_type VARCHAR(50) NOT NULL DEFAULT 'user', action VARCHAR(100) NOT NULL, resource_type VARCHAR(100) NOT NULL, resource_id VARCHAR(255), details JSONB, ip_address INET, user_agent TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW());

CREATE INDEX IF NOT EXISTS idx_athenos_audit_log_actor_id ON athenos_audit_log(actor_id);
CREATE INDEX IF NOT EXISTS idx_athenos_audit_log_action ON athenos_audit_log(action);
CREATE INDEX IF NOT EXISTS idx_athenos_audit_log_resource_type ON athenos_audit_log(resource_type);
CREATE INDEX IF NOT EXISTS idx_athenos_audit_log_created_at ON athenos_audit_log(created_at);

CREATE TABLE IF NOT EXISTS athenos_email_queue (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), to_email VARCHAR(255) NOT NULL, subject VARCHAR(500) NOT NULL, body TEXT NOT NULL, template VARCHAR(100), template_data JSONB, status VARCHAR(50) NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0, last_attempt_at TIMESTAMPTZ, error_message TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), scheduled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), CONSTRAINT chk_email_status CHECK (status IN ('pending', 'sent', 'failed', 'cancelled')));

CREATE INDEX IF NOT EXISTS idx_athenos_email_queue_status ON athenos_email_queue(status);
CREATE INDEX IF NOT EXISTS idx_athenos_email_queue_scheduled_at ON athenos_email_queue(scheduled_at);

CREATE TABLE IF NOT EXISTS athenos_setup_token (id INTEGER PRIMARY KEY DEFAULT 1, token_hash VARCHAR(255) NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), expires_at TIMESTAMPTZ NOT NULL, used_at TIMESTAMPTZ, CONSTRAINT single_setup_row CHECK (id = 1));

CREATE TABLE IF NOT EXISTS athenos_login_challenges (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), email TEXT NOT NULL, user_id UUID REFERENCES athenos_users(id) ON DELETE SET NULL, purpose TEXT NOT NULL CHECK (purpose IN ('login', 'register', 'password_reset')), pin_hash TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), expires_at TIMESTAMPTZ NOT NULL, consumed_at TIMESTAMPTZ, ip_address TEXT, user_agent TEXT);

CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_email ON athenos_login_challenges(email);
CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_user_id ON athenos_login_challenges(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_expires_at ON athenos_login_challenges(expires_at);
