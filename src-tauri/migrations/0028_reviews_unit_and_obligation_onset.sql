-- 0028: reviews as a conversation unit + role-obligation open-gating (ADR 0033).
--
-- ADR 0033 collapses the PR row to one attention dot. Two changes need schema:
-- a formal review body can @-mention you (a new conversation unit peer to the
-- general stream), and role obligations now clear when you open the PR (gated
-- on an onset watermark, re-arming on a fresh obligation).

-- Per-review @-mention bit, set by the per-cycle mention scanner over review
-- bodies (mirrors review_comments / issue_comments in migration 0025). Feeds the
-- reviews-unit branch (E) of the needs_attention roll-up.
ALTER TABLE reviews ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;

-- Per-PR reviews-stream seen watermark, peer to general_stream_seen_at. The
-- reviews unit clears when this advances (Reviews-tab "Mark all seen" / dwell).
ALTER TABLE pull_request_viewer_relations ADD COLUMN reviews_seen_at INTEGER;

-- Review-request onset: when the viewer was asked to review. The review-owed
-- obligation (branch C) is lit while the request is newer than the open
-- watermark (requested_at > read_at) and clears on open (read_at advances past
-- it); a fresh request re-arms it. The companion CHANGES_REQUESTED obligation
-- (branch D) reuses reviews.submitted_at as its onset, so it needs no column.
--
-- The sync worker preserves this across the per-cycle requested_reviewers
-- wipe-rewrite (kept for a login already present, set to the cycle clock for a
-- new one). Backfilled to the PR's created_at for rows that predate this
-- migration: a currently-requested reviewer who has not opened the PR
-- (read_at IS NULL) keeps the dot, while one who has opened it
-- (read_at > created_at) clears it - the closest sane approximation without a
-- historical request timestamp.
ALTER TABLE requested_reviewers ADD COLUMN requested_at INTEGER;
UPDATE requested_reviewers
   SET requested_at = (
       SELECT pr.created_at FROM pull_requests pr
        WHERE pr.id = requested_reviewers.pull_request_id
   )
 WHERE requested_at IS NULL;
