-- Migration: Create Athenos Password Reset Tokens Table
-- Version: 20260129
-- Description: Tokens for password reset functionality

CREATE TABLE IF NOT EXISTS athenos_password_reset_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES athenos_users(id) ON DELETE CASCADE,
    token_hash VARCHAR(255) NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_user_id ON athenos_password_reset_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_token_hash ON athenos_password_reset_tokens(token_hash);
CREATE INDEX IF NOT EXISTS idx_athenos_password_reset_tokens_expires_at ON athenos_password_reset_tokens(expires_at);
