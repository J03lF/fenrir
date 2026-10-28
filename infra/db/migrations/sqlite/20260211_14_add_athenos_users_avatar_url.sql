-- Migration: Add avatar_url to Athenos Users (SQLite)
-- Version: 20260211

ALTER TABLE athenos_users ADD COLUMN avatar_url TEXT;
