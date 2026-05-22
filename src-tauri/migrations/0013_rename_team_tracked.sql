-- v13 schema: rename the `repos.is_team_tracked` column to `is_tracked`.
--
-- The "Team" view is a misnomer (issue #220): it has no link to GitHub Teams,
-- team membership, or `team-review-requested` filters. The actual mechanic is
-- per-repo opt-in, picked by the user in Settings -> Repositories. M8 lands
-- an actual Teams-driven view; this migration renames the current column so
-- the two can coexist cleanly post-M8.
--
-- SQLite >= 3.25 supports `ALTER TABLE ... RENAME COLUMN` directly, preserving
-- row data and any partial indexes / views that reference the column. rusqlite
-- ships with a recent build, so no copy-table dance is needed.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE repos RENAME COLUMN is_team_tracked TO is_tracked;
