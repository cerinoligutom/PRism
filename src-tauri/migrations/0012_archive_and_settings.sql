-- v12 schema: archive bucket per-(account, PR) and the singleton
-- `app_settings` row that holds notification preferences. Foundation for
-- M6 - see ADR 0017 (desktop notifications) and ADR 0018 (archive + TTL).
--
-- The `archived_at` column extends the per-account triage row pattern
-- established by migration 0010 (read-state) and ADR 0015: every M6
-- archive write lives next to the read-state on the same relation row, so
-- the dashboard query keeps reading one join per render.
--
-- `app_settings` is a singleton: one row pinned at id = 1 via a CHECK
-- constraint. Notification preferences live app-wide (not per-account)
-- per ADR 0017; the singleton shape buys typed reads from Rust without a
-- JSON parse and keeps future settings additions cheap (one column per
-- toggle).
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

-- Unix seconds when the (account, PR) row was archived. NULL means the
-- row is not archived; the default-view dashboard queries add
-- `AND rel.archived_at IS NULL` to hide archived rows. Auto-archive
-- writes from the sync sweep (closed/merged + 30 days inactive); manual
-- archive writes from the PR row overflow menu. Both share this single
-- column - "archive" means the same thing whether the user clicked it or
-- the sweep set it. Unarchive sets back to NULL.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN archived_at INTEGER;

-- Partial index sized for the sweep and the Archive view. Only the
-- not-NULL subset is indexed - smaller index footprint, faster scans
-- against the archived bucket, no maintenance cost for the common case
-- where most rows are unarchived. See ADR 0018 for the query shapes that
-- read against this column.
CREATE INDEX idx_relations_archived_at
    ON pull_request_viewer_relations (archived_at)
    WHERE archived_at IS NOT NULL;

-- App-wide settings singleton. Pinned at id = 1 via a CHECK so a second
-- row can never be inserted by accident; the migration also seeds the
-- canonical row so reads never need to handle the empty case. Per ADR
-- 0017:
--   - `notifications_enabled` is the master switch (default OFF; flips
--     ON when the user accepts the OS permission prompt or explicitly
--     enables in Settings).
--   - `notify_on_needs_attention` / `notify_on_mention` are per-trigger
--     toggles, gated behind the master switch in the panel (default ON
--     once master is ON).
--   - `notification_permission_state` tracks the OS-level grant
--     (`unprompted`, `granted`, `denied`) so the Settings panel renders
--     the right call-to-action without re-asking the OS every time.
CREATE TABLE app_settings (
    id                              INTEGER PRIMARY KEY CHECK (id = 1),
    notifications_enabled           INTEGER NOT NULL DEFAULT 0,
    notify_on_needs_attention       INTEGER NOT NULL DEFAULT 1,
    notify_on_mention               INTEGER NOT NULL DEFAULT 1,
    notification_permission_state   TEXT    NOT NULL DEFAULT 'unprompted',
    updated_at                      INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

INSERT INTO app_settings (id) VALUES (1);
