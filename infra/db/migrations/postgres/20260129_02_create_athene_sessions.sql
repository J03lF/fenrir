-- Migration: Create Athene Sessions Table
-- Version: 20260129
-- Description: User sessions for authentication

CREATE TABLE IF NOT EXISTS athene_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES athene_users(id) ON DELETE CASCADE,
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
CREATE INDEX IF NOT EXISTS idx_athene_sessions_user_id ON athene_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_athene_sessions_token_hash ON athene_sessions(token_hash);
CREATE INDEX IF NOT EXISTS idx_athene_sessions_status ON athene_sessions(status);
CREATE INDEX IF NOT EXISTS idx_athene_sessions_expires_at ON athene_sessions(expires_at);
