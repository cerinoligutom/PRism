-- v20 schema: configurable auto-archive retention window on the
-- `app_settings` singleton. Issue #333 surfaces ADR-0018's 30-day TTL as
-- a user-tunable setting; the column defaults to 30 so existing installs
-- preserve today's behaviour, and the CHECK clamps the persisted value
-- to [0, 365] so writes can't smuggle a wider window past the writer's
-- own clamp.
--
-- Semantics:
--   * `auto_archive_days = 0` disables the auto-archive sweep entirely
--     (manual archive only). The worker short-circuits before issuing
--     the UPDATE, so 0 is a true no-op, not a "now - 0 days" predicate
--     that would archive every closed PR immediately.
--   * `auto_archive_days > 0` adds a `pull_requests.updated_at < now - N
--     days` gate to the sweep so closed / merged PRs linger in the
--     default views for the inactivity window before flipping to the
--     Archive bucket.
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

ALTER TABLE app_settings
    ADD COLUMN auto_archive_days INTEGER NOT NULL DEFAULT 30
        CHECK (auto_archive_days BETWEEN 0 AND 365);
