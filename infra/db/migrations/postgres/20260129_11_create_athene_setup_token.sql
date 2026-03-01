-- Migration: Create Athene Setup Token Table
-- Version: 20260129
-- Description: One-time setup token for initial admin creation

CREATE TABLE IF NOT EXISTS athene_setup_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    used_by UUID REFERENCES athene_users(id)
);

-- Index
CREATE INDEX IF NOT EXISTS idx_athene_setup_tokens_token_hash ON athene_setup_tokens(token_hash);
