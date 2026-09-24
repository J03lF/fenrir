-- Migration: Recover Athene Email Queue
-- Version: 20260528_01
-- Description: One-off recovery for environments where the email queue
--              contains rows that cause `recover_inflight_deliveries` to
--              fail at boot with "trailing input" or similar parse errors.
--
-- Background:
--   `notification-service` calls `email_repo.list()` on startup, which
--   loads every row and parses `template_data` via serde. A single
--   un-parseable row kills the boot before the HTTP listener binds,
--   leaving the service unreachable and Auth/Login hanging on the missing
--   notification target.
--
-- Strategy (defensive, idempotent):
--   1. Recover orphaned `sending` rows (a service died mid-delivery) by
--      flipping them to `failed` with a clear reason. The normal
--      recover-on-boot path does the same thing, but we do it in SQL
--      here so a brand-new boot doesn't trip the parse step.
--   2. Drop everything that's not a *successfully* delivered email
--      (`status = 'sent'`). Pending / failed / sending entries are
--      best-effort and not worth preserving across a recovery; sent rows
--      stay as an audit trail.
--
-- Safe to re-run.

UPDATE athene_email_queue
   SET status = 'failed',
       error  = COALESCE(NULLIF(error, ''), 'recovered after interrupted shutdown')
 WHERE status = 'sending';

DELETE FROM athene_email_queue
 WHERE status IN ('queued', 'sending', 'failed');
