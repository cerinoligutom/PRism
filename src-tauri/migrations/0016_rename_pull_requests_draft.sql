-- v16 schema: rename `pull_requests.draft` to `pull_requests.is_draft` so the
-- column name matches the project's `is_*` boolean predicate convention
-- (`is_resolved`, `is_outdated`, `is_tracked`). The Rust struct field has
-- always been `is_draft`; only the column name drifted from 0001.
--
-- SQLite >= 3.25 supports `ALTER TABLE ... RENAME COLUMN` directly, preserving
-- row data and any indexes / views that reference the column. Same mechanism
-- used by 0013 to rename `repos.is_team_tracked` to `repos.is_tracked`.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE pull_requests RENAME COLUMN draft TO is_draft;
