-- Migration: Add maintenance_until to Athenos App Settings (SQLite)
-- Version: 20260129

ALTER TABLE athenos_app_settings
    ADD COLUMN maintenance_until TEXT;
