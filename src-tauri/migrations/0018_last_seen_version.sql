-- v18 schema: add `last_seen_version` to the `app_settings` singleton so the
-- frontend can decide whether to surface the in-app "What's new" dialog on
-- launch. ADR 0025 records the decision: bundled `CHANGELOG.md`, single
-- concatenated dialog, first-install suppression via `NULL`.
--
-- The default is `NULL` (sentinel for "fresh install, no cursor yet"). The
-- frontend's launch hook writes `app_metadata.version` on first run so the
-- next version transition is what actually triggers the dialog, per the
-- decision in ADR 0025. A dedicated `set_last_seen_version` command writes
-- the column from the dialog dismiss handler; `update_app_settings` leaves
-- it alone (mirroring the `notification_permission_state` pattern in ADR
-- 0017 decision 5).
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

ALTER TABLE app_settings
    ADD COLUMN last_seen_version TEXT;
