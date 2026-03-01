-- Migration: Create Athene Login Attempts Table
-- Version: 20260129
-- Description: Audit log for login attempts

CREATE TABLE IF NOT EXISTS athene_login_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL,
    ip_address INET,
    user_agent TEXT,
    success BOOLEAN NOT NULL,
    failure_reason VARCHAR(255),
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_email ON athene_login_attempts(email);
CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_ip_address ON athene_login_attempts(ip_address);
CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_attempted_at ON athene_login_attempts(attempted_at);
CREATE INDEX IF NOT EXISTS idx_athene_login_attempts_success ON athene_login_attempts(success);

-- Cleanup old entries (keep 30 days)
-- This can be run periodically by a scheduled job
-- DELETE FROM athene_login_attempts WHERE attempted_at < NOW() - INTERVAL '30 days';
