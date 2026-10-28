-- Migration: Add maintenance_until to Athenos App Settings (Postgres)
-- Version: 20260129

ALTER TABLE athenos_app_settings
    ADD COLUMN IF NOT EXISTS maintenance_until TIMESTAMPTZ;
