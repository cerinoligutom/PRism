-- v5 schema: four-bucket threads rollup on pull_requests.
--
-- The M3 row threads bar split threads into three counts (unresolved,
-- resolved, involved) where `involved` overlapped both states. The redesign
-- collapses the bar into four disjoint buckets keyed on (resolved x
-- involved) so the dashboard row and conversation surface render identical
-- segments. See docs/contracts/conversation-depth.md and ADR 0012.
--
-- Outdated threads are now counted in the bar denominator and sort into
-- the same four buckets by their (resolved x involved) state. The
-- `threads_outdated` count is still surfaced by the stats tile but is
-- computed at stats-read time from `review_threads.is_outdated`; no
-- rollup column for it ever existed.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE pull_requests
    ADD COLUMN threads_unresolved_involved   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests
    ADD COLUMN threads_unresolved_uninvolved INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests
    ADD COLUMN threads_resolved_involved     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests
    ADD COLUMN threads_resolved_uninvolved   INTEGER NOT NULL DEFAULT 0;

-- Retire the v4 rollup columns. `threads_unresolved` and `threads_involved`
-- mapped state and involvement onto overlapping counts; the four new
-- columns are disjoint. `threads_total` stays as the bar denominator.
ALTER TABLE pull_requests DROP COLUMN threads_unresolved;
ALTER TABLE pull_requests DROP COLUMN threads_involved;
