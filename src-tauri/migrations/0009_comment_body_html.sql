-- v9 schema: persist GitHub's pre-rendered HTML for review comments, issue
-- comments, and review summaries. The lazy hydrator and the sync cycle write
-- the new columns alongside the existing `body` markdown text; the frontend
-- renders the HTML through `PRismMarkdown` (DOMPurify sanitised + Shiki for
-- code blocks) and falls back to plain `body` when NULL. See ADR 0014 and
-- issue #138.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE review_comments ADD COLUMN body_html TEXT;
ALTER TABLE issue_comments ADD COLUMN body_html TEXT;
ALTER TABLE reviews ADD COLUMN body_html TEXT;
