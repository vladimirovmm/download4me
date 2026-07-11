-- Add up migration script here
ALTER TABLE sites
ADD COLUMN proxy INTEGER NOT NULL DEFAULT 1;
