-- v15 schema: index `review_comments.author_login` to back the dashboard
-- `thread_buckets` involvement test (ADR 0016). The subquery does four
-- `JOIN accounts a ON a.login = c.author_login` lookups per PR row - one
-- per resolved/involved bucket - so without an index every involvement
-- EXISTS becomes a full scan of `review_comments`. See issue #231.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

CREATE INDEX idx_review_comments_author_login
    ON review_comments (author_login);
