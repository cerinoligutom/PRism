-- v24 schema: drop the `review_threads.head_comment_*` denorm columns.
-- ADR 0029 made the sync worker the canonical writer for `review_comments`,
-- so the per-thread head-comment author / body / created_at columns that
-- existed as a workaround for the empty-comments table are now redundant.
-- The conversation read query derives the head comment from
-- `review_comments ORDER BY created_at ASC LIMIT 1` instead.
--
-- SQLite 3.35+ supports inline `ALTER TABLE DROP COLUMN`; rusqlite 0.39 with
-- the `bundled` feature ships a bundled SQLite well above that threshold.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE review_threads DROP COLUMN head_comment_author_login;
ALTER TABLE review_threads DROP COLUMN head_comment_body_text;
ALTER TABLE review_threads DROP COLUMN head_comment_created_at;
