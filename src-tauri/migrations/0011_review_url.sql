-- GitHub permalink for review summaries (PullRequestReview.url). Mirrors
-- `review_threads.url` from migration 0007: surfaced so the conversation
-- surface can offer a per-review "Open in GitHub" affordance on the Reviews
-- tab. Rows written before this migration carry NULL; the frontend hides
-- the button until the next sync cycle backfills the URL.

ALTER TABLE reviews ADD COLUMN url TEXT;
