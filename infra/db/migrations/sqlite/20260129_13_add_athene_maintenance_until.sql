-- Migration: Add maintenance_until to Athene App Settings (SQLite)
-- Version: 20260129

ALTER TABLE athene_app_settings
    ADD COLUMN maintenance_until TEXT;
