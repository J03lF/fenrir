-- Migration: Create Athene API Keys Table
-- Version: 20260129
-- Description: API keys for authentication and access control

CREATE TABLE IF NOT EXISTS athene_api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    key_hash VARCHAR(255) NOT NULL UNIQUE,
    key_prefix VARCHAR(20) NOT NULL,
    user_id UUID REFERENCES athene_users(id) ON DELETE SET NULL,
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    scopes TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_by UUID NOT NULL REFERENCES athene_users(id),
    
    CONSTRAINT chk_api_key_status CHECK (status IN ('active', 'revoked', 'expired'))
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_key_hash ON athene_api_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_status ON athene_api_keys(status);
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_user_id ON athene_api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_athene_api_keys_created_by ON athene_api_keys(created_by);
