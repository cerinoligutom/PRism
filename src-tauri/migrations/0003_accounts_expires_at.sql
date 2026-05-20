-- v3 schema: surface PAT expiry on the accounts row so the auth + sync layers
-- can flag soon-to-expire tokens. Filled by `add_account` from GitHub's
-- `github-authentication-token-expiration` response header; NULL when the PAT
-- has no expiry (classic, or fine-grained without one set).
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE accounts ADD COLUMN expires_at TEXT;
