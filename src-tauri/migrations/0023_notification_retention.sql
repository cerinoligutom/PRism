-- v23 schema: configurable count cap for the persistent notifications inbox
-- (ADR 0028 retention decision, issue #380). The `notifications` table is a
-- higher-velocity record than PRs - one row per dispatched toast - so an
-- unbounded table is a real failure mode at v1 scale. A count cap gives every
-- user the same "last N" experience independent of activity and prunes
-- cheaply on insert.
--
-- Semantics:
--   * `notification_retention_max` caps the row count in `notifications`.
--     The store's `prune_to_cap` runs after every insert in
--     `notifications::store::insert` and deletes any row whose id is not
--     among the newest N (ordered by `created_at DESC, id DESC`).
--   * The default of 500 keeps the table around ~250 KB at the upper bound
--     (500 rows x ~500 bytes), well inside v1's DB-size envelope.
--   * The Settings UI clamps writes to [50, 5000]; the CHECK here mirrors
--     the same bound so a write that bypasses the writer's clamp can't
--     smuggle an out-of-range value into the column.
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

ALTER TABLE app_settings
    ADD COLUMN notification_retention_max INTEGER NOT NULL DEFAULT 500
        CHECK (notification_retention_max BETWEEN 50 AND 5000);
