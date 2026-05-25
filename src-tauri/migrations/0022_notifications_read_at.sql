-- v22 schema: read/unread state on the persistent notifications inbox.
--
-- A nullable `read_at` timestamp doubles as a marker bit (NULL means unread)
-- and a "when did the user clear this signal" record (ADR 0028 decision 3).
-- The partial index keeps the unread count cheap as the table grows toward
-- the count cap (#380).
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

ALTER TABLE notifications ADD COLUMN read_at INTEGER;

CREATE INDEX idx_notifications_unread
    ON notifications (created_at DESC)
    WHERE read_at IS NULL;
