-- Migration: Add maintenance_until to Athene App Settings (Postgres)
-- Version: 20260129

ALTER TABLE athene_app_settings
    ADD COLUMN IF NOT EXISTS maintenance_until TIMESTAMPTZ;
