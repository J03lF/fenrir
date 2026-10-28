-- Migration: Create Athenos Users Table
-- Version: 20260129
-- Description: User accounts for Athenos ticketing system

CREATE TABLE IF NOT EXISTS athenos_users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email VARCHAR(255) NOT NULL UNIQUE,
    email_verified BOOLEAN NOT NULL DEFAULT FALSE,
    display_name VARCHAR(255) NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    role VARCHAR(50) NOT NULL DEFAULT 'user',
    status VARCHAR(50) NOT NULL DEFAULT 'pending_verification',
    failed_login_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ,
    
    CONSTRAINT chk_role CHECK (role IN ('admin', 'operator', 'user', 'guest')),
    CONSTRAINT chk_status CHECK (status IN ('active', 'inactive', 'locked', 'pending_verification'))
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athenos_users_email ON athenos_users(email);
CREATE INDEX IF NOT EXISTS idx_athenos_users_status ON athenos_users(status);
CREATE INDEX IF NOT EXISTS idx_athenos_users_role ON athenos_users(role);

-- Trigger for updated_at
CREATE OR REPLACE FUNCTION update_athenos_users_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_athenos_users_updated_at ON athenos_users;
CREATE TRIGGER trg_athenos_users_updated_at
    BEFORE UPDATE ON athenos_users
    FOR EACH ROW
    EXECUTE FUNCTION update_athenos_users_updated_at();
