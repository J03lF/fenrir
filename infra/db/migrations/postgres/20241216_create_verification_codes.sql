CREATE TABLE IF NOT EXISTS verification_codes (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    pin_hash TEXT NOT NULL,
    issued_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed', 'expired'))
);

CREATE INDEX IF NOT EXISTS idx_verification_codes_email_status
    ON verification_codes (email, status);

CREATE INDEX IF NOT EXISTS idx_verification_codes_expires_at
    ON verification_codes (expires_at);
