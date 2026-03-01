-- Migration: Create Athene Audit Log Table
-- Version: 20260129
-- Description: Audit log for tracking admin actions and security events

CREATE TABLE IF NOT EXISTS athene_audit_log (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id UUID REFERENCES athene_users(id) ON DELETE SET NULL,
    actor_email VARCHAR(255),
    action VARCHAR(100) NOT NULL,
    resource_type VARCHAR(100) NOT NULL,
    resource_id VARCHAR(255),
    details JSONB,
    ip_address INET,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_actor_id ON athene_audit_log(actor_id);
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_action ON athene_audit_log(action);
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_resource_type ON athene_audit_log(resource_type);
CREATE INDEX IF NOT EXISTS idx_athene_audit_log_created_at ON athene_audit_log(created_at);

-- Partition by month (optional, for high-volume deployments)
-- This is a comment showing how to partition if needed in the future
