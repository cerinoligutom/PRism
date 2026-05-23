-- v17 schema: rename `pull_request_viewer_relations.last_seen_at` to
-- `relation_observed_at` so the sync-cycle bookkeeping timestamp doesn't
-- collide visually with `read_at` (added in 0010), which records when the
-- *user* last opened the PR. 0010's own narrative comment acknowledges the
-- overlap was a known wart at the time.
--
-- The other `last_seen_at` columns on `etags` (0001) and `users` (0006) are
-- intentionally untouched - they sit on different tables, don't share a
-- table with `read_at`, and the bookkeeping vs user-action distinction
-- doesn't apply there.
--
-- SQLite >= 3.25 supports `ALTER TABLE ... RENAME COLUMN` in place,
-- preserving row data and any indexes referencing the column. Same
-- mechanism used by 0013 and 0016.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE pull_request_viewer_relations
    RENAME COLUMN last_seen_at TO relation_observed_at;
