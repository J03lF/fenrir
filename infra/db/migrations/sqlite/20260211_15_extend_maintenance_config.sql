-- Extend maintenance configuration with force-logout scheduling, dismiss cooldown and warning time.
ALTER TABLE athene_app_settings ADD COLUMN maintenance_force_logout_at TEXT;
ALTER TABLE athene_app_settings ADD COLUMN maintenance_dismiss_seconds INTEGER NOT NULL DEFAULT 30;
ALTER TABLE athene_app_settings ADD COLUMN maintenance_warn_minutes INTEGER NOT NULL DEFAULT 5;
