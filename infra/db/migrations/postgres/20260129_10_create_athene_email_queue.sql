-- Migration: Create Athene Email Queue Table
-- Version: 20260129
-- Description: Email queue for notification service

CREATE TABLE IF NOT EXISTS athene_email_queue (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    recipient VARCHAR(255) NOT NULL,
    subject VARCHAR(500) NOT NULL,
    body_html TEXT NOT NULL,
    body_text TEXT,
    template_id VARCHAR(100),
    template_data JSONB,
    status VARCHAR(50) NOT NULL DEFAULT 'queued',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    sent_at TIMESTAMPTZ,
    error TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0,
    
    CONSTRAINT chk_email_status CHECK (status IN ('queued', 'sending', 'sent', 'failed'))
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_athene_email_queue_status ON athene_email_queue(status);
CREATE INDEX IF NOT EXISTS idx_athene_email_queue_created_at ON athene_email_queue(created_at);
CREATE INDEX IF NOT EXISTS idx_athene_email_queue_recipient ON athene_email_queue(recipient);
