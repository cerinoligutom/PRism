-- v8 schema: persist the GitHub permalink for each review and issue comment.
-- The thread permalink kept in `review_threads.url` (added in migration 0007)
-- is now derived at write time from the head comment's `url`, because
-- `PullRequestReviewThread` doesn't expose a `url` field on GitHub's GraphQL
-- schema. See issue #115.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE review_comments ADD COLUMN url TEXT;
ALTER TABLE issue_comments ADD COLUMN url TEXT;
