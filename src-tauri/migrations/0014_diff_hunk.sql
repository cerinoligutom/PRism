-- v14 schema: persist GitHub's `diffHunk` for each review thread so the
-- conversation surface can render the file-context diff above each thread
-- card. The hunk lives at the comment level on GitHub but every comment
-- in a thread carries the same value (it's the code context the thread is
-- about), so the column sits on `review_threads` and the lazy hydrator
-- writes it once per thread from the head comment. See issue #162.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE review_threads ADD COLUMN diff_hunk TEXT;
