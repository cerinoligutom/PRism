-- v4 schema: conversation depth. See docs/contracts/conversation-depth.md and
-- ADR 0010.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

-- ----------------------------------------------------------------
-- review_threads: per-thread state needed by the threads list and
-- the conversation-stats math.
-- ----------------------------------------------------------------

-- GraphQL node id - required for upserts (ReviewThread has no databaseId).
ALTER TABLE review_threads ADD COLUMN node_id              TEXT;

-- Outdated state - surfaced in the threads list; counted in total but not
-- in unresolved.
ALTER TABLE review_threads ADD COLUMN is_outdated          INTEGER NOT NULL DEFAULT 0;

-- Timestamps (unix seconds) needed by the conversation stats.
ALTER TABLE review_threads ADD COLUMN created_at           INTEGER;
ALTER TABLE review_threads ADD COLUMN resolved_at          INTEGER;
ALTER TABLE review_threads ADD COLUMN last_reply_at        INTEGER;

-- Reply count - denormalised from review_comments so the list query
-- doesn't need a sub-aggregation.
ALTER TABLE review_threads ADD COLUMN reply_count          INTEGER NOT NULL DEFAULT 0;

-- Head-comment snapshot - first comment in the thread, surfaced as the
-- preview snippet on the threads list. Populated from the cycle's
-- `comments(first:1)` head; full bodies live in review_comments after
-- lazy hydration.
ALTER TABLE review_threads ADD COLUMN head_comment_author_login   TEXT;
ALTER TABLE review_threads ADD COLUMN head_comment_body_text      TEXT;
ALTER TABLE review_threads ADD COLUMN head_comment_created_at     INTEGER;

-- Line range (single line or multi-line block comment).
ALTER TABLE review_threads ADD COLUMN line                  INTEGER;
ALTER TABLE review_threads ADD COLUMN start_line            INTEGER;
-- `original_line` already exists from 0001_init.sql.

CREATE UNIQUE INDEX idx_review_threads_node_id
    ON review_threads (node_id)
    WHERE node_id IS NOT NULL;

-- Threads list queries filter by PR + (resolved OR outdated) - partial index
-- on the unresolved-and-active set keeps the threads list fast.
CREATE INDEX idx_review_threads_pr_active
    ON review_threads (pull_request_id)
    WHERE is_resolved = 0 AND is_outdated = 0;

-- ----------------------------------------------------------------
-- review_comments: lazy-hydrated per-thread comment bodies.
-- ----------------------------------------------------------------

-- GraphQL node id + REST databaseId - either form may upsert depending
-- on which lazy-fetch path produced the row.
ALTER TABLE review_comments ADD COLUMN node_id              TEXT;
ALTER TABLE review_comments ADD COLUMN database_id          INTEGER;

-- Line + side (LEFT / RIGHT) for inline rendering. Mostly informational
-- in M3 (no diff viewer); persisted so M4+ can use them without backfill.
ALTER TABLE review_comments ADD COLUMN line                 INTEGER;
ALTER TABLE review_comments ADD COLUMN side                 TEXT;

CREATE UNIQUE INDEX idx_review_comments_node_id
    ON review_comments (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_review_comments_thread
    ON review_comments (review_thread_id, created_at);

-- ----------------------------------------------------------------
-- issue_comments: lazy-hydrated PR-level comment bodies.
-- ----------------------------------------------------------------

ALTER TABLE issue_comments ADD COLUMN node_id              TEXT;
ALTER TABLE issue_comments ADD COLUMN database_id          INTEGER;

CREATE UNIQUE INDEX idx_issue_comments_node_id
    ON issue_comments (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_issue_comments_pr
    ON issue_comments (pull_request_id, created_at);

-- ----------------------------------------------------------------
-- reviews: each submitted PullRequestReview (state + body).
-- ----------------------------------------------------------------

ALTER TABLE reviews ADD COLUMN node_id                     TEXT;

CREATE UNIQUE INDEX idx_reviews_node_id
    ON reviews (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_reviews_pr_submitted_at
    ON reviews (pull_request_id, submitted_at);

-- ----------------------------------------------------------------
-- pull_requests: rollup columns for the dashboard row (cheap to
-- aggregate at write time; mirrors M2 ci_total / ci_passing).
-- ----------------------------------------------------------------

ALTER TABLE pull_requests ADD COLUMN threads_total         INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN threads_unresolved    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN threads_involved      INTEGER NOT NULL DEFAULT 0;

-- Cycle-time counter for the issue_comments contribution to the
-- comment-type breakdown. Bodies are hydrated lazily but the count
-- is read every cycle from `totalCount`.
ALTER TABLE pull_requests ADD COLUMN issue_comments_count  INTEGER NOT NULL DEFAULT 0;
