-- v25 schema: conversation-unit read-state foundation (ADR 0031, issue #431,
-- part of #428). Unifies triage, the notifications inbox, and OS toasts onto a
-- per-conversation-unit engagement watermark. This slice is additive and
-- schema-only: it creates the tables and columns later slices read and write.
-- The new columns are unused until then, which is expected.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

-- Per-review-thread explicit "seen" watermark, keyed on the GraphQL node_id (durable
-- across a transient delete+re-add in a paginated fetch). account_id cascades; seen_at advances only.
CREATE TABLE thread_read_state (
    account_id            INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    review_thread_node_id TEXT    NOT NULL,
    seen_at               INTEGER NOT NULL,
    PRIMARY KEY (account_id, review_thread_node_id)
);

-- Per-PR general-stream seen watermark + per-PR dispatch dedup, on the relation row (cascade + join for free).
ALTER TABLE pull_request_viewer_relations ADD COLUMN general_stream_seen_at   INTEGER;
ALTER TABLE pull_request_viewer_relations ADD COLUMN last_emitted_activity_at INTEGER;

-- Inbox unit reference + deep link (per-row derived unread; deep-link the exact unit). All nullable.
ALTER TABLE notifications ADD COLUMN unit_kind     TEXT;  -- 'thread' | 'general' | NULL (legacy)
ALTER TABLE notifications ADD COLUMN unit_ref      TEXT;  -- review_thread_node_id, or NULL
ALTER TABLE notifications ADD COLUMN deep_link_url TEXT;

-- Per-comment mention bit set by the existing scanner; replaces mentioned_count_unread as the signal.
ALTER TABLE review_comments ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;
ALTER TABLE issue_comments  ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;

-- DEPRECATED, left vestigial (a SQLite column drop is a table rebuild for no payoff now):
--   pull_request_viewer_relations.mentioned_count_unread, .mention_scan_watermark_at,
--   settings.notify_on_mention. Stop reading them; a later cleanup ADR may drop them.
