-- Migration: Add avatar_url to Athenos Users
-- Version: 20260211
-- Description: Adds avatar_url column for user profile pictures

ALTER TABLE athenos_users
    ADD COLUMN IF NOT EXISTS avatar_url VARCHAR(2048);
