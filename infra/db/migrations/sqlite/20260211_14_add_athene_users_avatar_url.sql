-- Migration: Add avatar_url to Athene Users (SQLite)
-- Version: 20260211

ALTER TABLE athene_users ADD COLUMN avatar_url TEXT;
