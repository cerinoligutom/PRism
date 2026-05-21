-- v7 schema: persist the GitHub permalink for each review thread so the
-- conversation surface can offer an "Open in GitHub" action per thread without
-- reconstructing the URL client-side. See docs/contracts/conversation-depth.md
-- and issue #102.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE review_threads ADD COLUMN url TEXT;
