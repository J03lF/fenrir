-- Migration: Create Athenos Email Queue Table (SQLite)
-- Version: 20260129

CREATE TABLE IF NOT EXISTS athenos_email_queue (
    id TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))),
    recipient TEXT NOT NULL,
    subject TEXT NOT NULL,
    body_html TEXT NOT NULL,
    body_text TEXT,
    template_id TEXT,
    template_data TEXT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'sending', 'sent', 'failed')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    sent_at TEXT,
    error TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_athenos_email_queue_status ON athenos_email_queue(status);
CREATE INDEX IF NOT EXISTS idx_athenos_email_queue_created_at ON athenos_email_queue(created_at);
