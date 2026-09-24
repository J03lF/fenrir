-- Migration: Recover Athene Email Queue
-- Version: 20260528_01
-- Description: SQLite parallel of the postgres recovery migration. See
--              the postgres file for the full reasoning.

UPDATE athene_email_queue
   SET status = 'failed',
       error  = COALESCE(NULLIF(error, ''), 'recovered after interrupted shutdown')
 WHERE status = 'sending';

DELETE FROM athene_email_queue
 WHERE status IN ('queued', 'sending', 'failed');
