-- v27 schema: drop the dead pre-0031 notification remnants (ADR 0032, issue #451).
--
-- ADR 0031 collapsed the two per-trigger toggles onto `notify_on_needs_attention`
-- and replaced the standalone mention counter with the per-comment
-- `mentions_viewer` bit the roll-up folds into involvement. Two columns were
-- left vestigial at the time and are now genuinely unreferenced:
--
--   * app_settings.notify_on_mention - persisted and written, but never read for
--     any dispatch decision; its UI control was removed in #445.
--   * pull_request_viewer_relations.mentioned_count_unread - no longer read by the
--     roll-up, the dashboard DTO, or any surface; the mark-read / mark-view-read
--     resets that touched it are gone.
--
-- KEPT deliberately:
--   * pull_request_viewer_relations.mention_scan_watermark_at - still the active
--     scan-efficiency watermark: the sync worker reads it to bound the mention
--     scan and advances it every cycle. Not vestigial; do not drop.
--
-- SQLite 3.35+ supports inline `ALTER TABLE DROP COLUMN`; rusqlite 0.39 with the
-- `bundled` feature ships a bundled SQLite well above that threshold. Neither
-- column carries an index or view dependency.
--
-- Migrations are forward-only and never edited in place. Add new migrations as
-- `NNNN_description.sql` rather than amending this file.

ALTER TABLE app_settings DROP COLUMN notify_on_mention;
ALTER TABLE pull_request_viewer_relations DROP COLUMN mentioned_count_unread;
