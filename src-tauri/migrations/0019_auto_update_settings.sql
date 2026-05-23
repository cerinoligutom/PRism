-- v19 schema: auto-update preferences + last-check bookkeeping on the
-- `app_settings` singleton. ADR-0024 records the decision: opt-in
-- default, fixed 6h cadence, silent failure surfaced as a "Last check
-- failed" line in the Settings -> Updates panel.
--
-- All four columns live next to the existing sync + notification
-- settings so the worker boundary (ADR-0020) reads them through the
-- same singleton. `auto_update_enabled` and
-- `auto_update_interval_seconds` are written by
-- `update_app_settings`; the last-check fields are owned by the
-- updater module's worker via a dedicated `record_update_check`
-- command, mirroring the `notification_permission_state` /
-- `last_seen_version` ownership pattern.
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

-- Boolean as integer (0 / 1). Defaults to 0 per ADR-0024: an observer
-- tool shouldn't restart itself without consent, so the user has to
-- opt in via the Settings -> Updates toggle.
ALTER TABLE app_settings
    ADD COLUMN auto_update_enabled INTEGER NOT NULL DEFAULT 0;

-- Poll cadence in seconds. Defaults to 21600 (6 hours) per ADR-0024.
-- v1.x ships this fixed value; the column is here so a future
-- configurable-interval setting has the natural home without another
-- migration.
ALTER TABLE app_settings
    ADD COLUMN auto_update_interval_seconds INTEGER NOT NULL DEFAULT 21600;

-- Unix seconds of the last update check attempt (success or failure).
-- NULL means no check has ever run; the Settings panel renders nothing
-- in that case. Written by the updater worker on every poll, including
-- manual "Check now" presses.
ALTER TABLE app_settings
    ADD COLUMN auto_update_last_check_at INTEGER;

-- Short human-readable error message from the last failed check, or
-- NULL when the last check succeeded. The Settings panel surfaces
-- "Last check failed: <message>" iff this column is set. Cleared on
-- the next successful check.
ALTER TABLE app_settings
    ADD COLUMN auto_update_last_failure_message TEXT;
