-- Migration: Create Athenos Sessions Table
-- Version: 20260129
-- Description: User sessions for authentication

CREATE TABLE IF NOT EXISTS athenos_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES athenos_users(id) ON DELETE CASCADE,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    ip_address INET,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_activity TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    CONSTRAINT chk_session_status CHECK (status IN ('active', 'expired', 'revoked'))
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_user_id ON athenos_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_token_hash ON athenos_sessions(token_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_status ON athenos_sessions(status);
CREATE INDEX IF NOT EXISTS idx_athenos_sessions_expires_at ON athenos_sessions(expires_at);
