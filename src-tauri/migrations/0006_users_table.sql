-- v6 schema: GitHub user avatar cache. See docs/adr/0013-user-avatars-cache.md.
--
-- One row per GitHub login encountered by the sync cycle. The dashboard and
-- conversation read queries `LEFT JOIN users ON users.login = ...author_login`
-- to surface `avatar_url` alongside the author/actor login. Single source of
-- truth: every author-bearing table joins the same row regardless of which
-- account first saw the user.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

CREATE TABLE users (
    login          TEXT PRIMARY KEY,
    avatar_url     TEXT,
    -- Unix seconds. Updated on every upsert so a future eviction policy can
    -- drop logins not seen in N cycles. v1 keeps every row.
    last_seen_at   INTEGER NOT NULL DEFAULT 0
);
