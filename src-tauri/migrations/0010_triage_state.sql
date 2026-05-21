-- v10 schema: per-account triage state - read-watermarks, mention counters,
-- and the precomputed "needs my attention" composite flag. Layered on top of
-- the existing `pull_request_viewer_relations` table (M2 / migration 0002)
-- because every signal is per-(account, PR) and the relation row already
-- exists for every PR the viewer touches. See ADR 0015 and
-- `docs/contracts/triage-ux.md`.
--
-- Naming note: the table already carries a `last_seen_at` column populated by
-- the discovery / pruning phase to track relation freshness. The new read
-- watermarks use the `read_*` prefix so they never collide with the cycle
-- bookkeeping field.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

-- Unix seconds when the viewer last opened this PR's detail surface. NULL
-- means the PR has never been opened on this account - the row reads as
-- "unread" regardless of `pull_requests.updated_at`.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN read_at INTEGER;

-- Snapshot of `pull_requests.updated_at` at the moment `read_at` was set.
-- The frontend derives `unread` as
--     read_at IS NULL OR pull_requests.updated_at > read_pr_updated_at
-- so a sync cycle that bumps `updated_at` after the viewer last opened the
-- drawer flips the PR back to unread without writing a new row.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN read_pr_updated_at INTEGER;

-- Running count of `@<viewer-login>` matches the sync cycle has seen across
-- review-comment + issue-comment bodies that landed after the last `read_at`
-- watermark. Reset to 0 by `mark_pr_read`. Sync increments idempotently using
-- the per-comment created_at vs `mention_scan_watermark_at` (see below).
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN mentioned_count_unread INTEGER NOT NULL DEFAULT 0;

-- Unix seconds of the latest comment.created_at the mention scanner has
-- already counted into `mentioned_count_unread`. The next sync only scans
-- comments newer than this watermark so a re-run never double-counts.
-- Initialised to 0 so the first scan picks up every comment newer than the
-- epoch (and `mark_pr_read` pushes the watermark to the current cycle's
-- timestamp on every open, even when no mentions were present).
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN mention_scan_watermark_at INTEGER NOT NULL DEFAULT 0;

-- Precomputed "needs my attention" composite. Written by the sync worker
-- after every cycle (and by `mark_pr_read` / `mark_pr_unread` for the
-- mention-driven flip) so the dashboard query can read a single column
-- instead of running the composite formula at every list render. See ADR
-- 0015 ("Composite formula") for the definition of the four input
-- conditions.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN needs_attention INTEGER NOT NULL DEFAULT 0;

-- Partial index sized for the sidebar count-badge query
-- (`SELECT COUNT(*) ... WHERE account_id = ? AND needs_attention = 1`).
-- The full relations table is small but the predicate keeps the index
-- footprint to just the attention rows, which is the natural denominator
-- for the "Needs my attention" sidebar badge in every view.
CREATE INDEX idx_pr_viewer_relations_attention
    ON pull_request_viewer_relations (account_id)
    WHERE needs_attention = 1;
