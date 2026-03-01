-- Extend maintenance configuration with force-logout scheduling, dismiss cooldown and warning time.
ALTER TABLE athene_app_settings
  ADD COLUMN IF NOT EXISTS maintenance_force_logout_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS maintenance_dismiss_seconds INTEGER NOT NULL DEFAULT 30,
  ADD COLUMN IF NOT EXISTS maintenance_warn_minutes INTEGER NOT NULL DEFAULT 5;
