# 0032 - Prune the pre-0031 notification remnants

- **Status:** Accepted
- **Date:** 2026-05-31
- **Issue:** [#451](https://github.com/cerinoligutom/PRism/issues/451)
- **Deciders:** @cerinoligutom

## Context

ADR 0031 unified the three read-state systems onto one conversation-unit attention model. It collapsed the two per-trigger notification toggles onto `notify_on_needs_attention` and replaced the standalone `mentioned_count_unread` counter with a persisted per-comment `mentions_viewer` bit the roll-up folds into involvement. To keep that slice additive, 0031 left three columns vestigial rather than rebuilding the tables: `app_settings.notify_on_mention`, `pull_request_viewer_relations.mentioned_count_unread`, and `pull_request_viewer_relations.mention_scan_watermark_at`. Its "Open follow-ups" called for a cleanup ADR to drop the genuinely-dead ones.

A code-level audit of the three columns settled which are dead and which are still load-bearing:

- `notify_on_mention` is persisted and written but never read for any dispatch decision; `decide_dispatch` reads only `notifications_enabled` and `notify_on_needs_attention`. Its UI control was removed in #445. Dead.
- `mentioned_count_unread` is no longer read by the roll-up (which reads `mentions_viewer`), by the dashboard DTO, or by any rendered surface. It was still reset by `mark_read` / `mark_view_read` and bumped by the mention scan, but nothing consumed those writes. Dead once those writes are removed.
- `mention_scan_watermark_at` is the active scan-efficiency watermark: the sync worker reads it to bound the mention scan to comments newer than the last cycle and advances it every cycle. Not vestigial - 0031's schema comment listing it alongside the dead columns was wrong (corrected by this ADR's dated note on 0031).

## Decision drivers

- Remove dead schema and the code that touches it so the next reader isn't misled about what the columns mean.
- Keep the change mechanical: no behaviour change a user can observe, only the removal of writes/reads nothing consumed.
- Don't drop a column that is still doing work. Confirm zero live reads before the migration runs.
- Stay diff-able against the design source: the CSS / token mirrors are not ours to prune here.

## Considered options

1. **Drop `notify_on_mention` and `mentioned_count_unread`; keep `mention_scan_watermark_at`.** Matches the audit: drop exactly what is dead, keep what the scanner relies on.
2. **Drop all three "vestigial" columns 0031 named.** Wrong - `mention_scan_watermark_at` is live; dropping it would re-scan every comment from the epoch each cycle.
3. **Leave everything; just fix the comments.** Keeps dead columns and dead code around indefinitely, which is the rot 0031's follow-up asked to clear.

## Decision

We will go with **option 1**.

Migration `0027_drop_dead_notification_columns.sql` issues `ALTER TABLE app_settings DROP COLUMN notify_on_mention` and `ALTER TABLE pull_request_viewer_relations DROP COLUMN mentioned_count_unread`. Neither column carries an index or view dependency, and the bundled SQLite is well above the 3.35 `DROP COLUMN` threshold (the same path migration 0024 used). All reads and writes of the two columns are removed first so the migration drops columns with no remaining references:

- `notify_on_mention`: removed from the `AppSettings` struct + its `SELECT`, from `update_app_settings`'s `UPDATE`, from the notification-sink test writer, and from the frontend `AppSettings` / `AppSettingsUpdate` types, the settings store's threading, and the three settings views that round-tripped it. Its `decide_dispatch` comment is corrected to say the column was dropped (it was never read there).
- `mentioned_count_unread`: removed from the dashboard DTO (Rust + TS), the projection `SELECT` / `SUM` and the row read, the `signalsFixtures` fixture, and the `mark_read` / `mark_view_read` resets. The mention scan no longer bumps it (it still sets the `mentions_viewer` bit). The `hasUnreadInView` dashboard gate drops its stale `|| pr.mentioned_count_unread > 0` term - mentions are an attention signal now, not an unread-axis signal, and "Mark all read" works the unread axis.

### Kept deliberately

- `mention_scan_watermark_at` stays. It is the scanner's idempotency cursor (`triage_recompute`), read to bound the scan and advanced every cycle. This ADR adds a dated note to 0031 correcting its schema comment, which had grouped this column with the dead ones.
- The `row-strip-*` rules in `primitives.css` and the `--attention-tint*` tokens in `tokens.css` stay. Per CLAUDE.md, `primitives.css` mirrors the design source (`docs/design/app.css`) verbatim and is treated as a consumed utility layer, not extended or pruned; the tokens back it. Pruning them here would break that diff-ability for no schema payoff. No app-level (BEM) CSS referenced the dropped columns, so nothing else needed removing.

## Consequences

### Positive

- The schema and the code match the 0031 model: one mention signal (`mentions_viewer`), one notification toggle (`notify_on_needs_attention`), no dead columns to mislead the next reader.
- `mark_read` / `mark_view_read` and the mention scan each do one fewer write.

### Negative

- A forward-only column drop: a downgrade to a pre-0027 binary would find the columns gone. Consistent with PRism's forward-only migration policy; no rollback path is offered for any migration.

### Neutral / follow-ups

- Tests that asserted the dropped columns' reset / persist / SUM-merge behaviour are removed or retargeted at the surviving signal (`needs_attention`, `mentions_viewer`); the mention-scan count tests now count `mentions_viewer` bits, which the scan sets on the same matched comments.

## References

- Prunes the remnants left by ADR [0031](0031-conversation-unit-attention-and-rearm-dispatch.md) (its "cleanup ADR to drop the vestigial 0010 columns and the mention toggle" follow-up).
- Builds on the column-drop pattern in migration `0024_drop_review_thread_head_denorm.sql`.
- Migration: `src-tauri/migrations/0027_drop_dead_notification_columns.sql`.
- Vestigial columns originally added by ADR [0015](0015-triage-state-model.md) (migration 0010) and the settings toggle by migration 0012.
