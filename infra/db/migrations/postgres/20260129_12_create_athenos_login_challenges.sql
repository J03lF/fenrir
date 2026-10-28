-- Migration: Create Athenos Login Challenges Table (Postgres)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athenos_login_challenges (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL,
    user_id UUID REFERENCES athenos_users(id) ON DELETE SET NULL,
    purpose TEXT NOT NULL CHECK (purpose IN ('login', 'register', 'password_reset')),
    pin_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    ip_address TEXT,
    user_agent TEXT
);

CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_email ON athenos_login_challenges(email);
CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_user_id ON athenos_login_challenges(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_login_challenges_expires_at ON athenos_login_challenges(expires_at);
