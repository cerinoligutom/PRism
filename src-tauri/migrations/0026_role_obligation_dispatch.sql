-- v26 schema: per-PR role-obligation dispatch dedup (ADR 0031 amendment,
-- issue #450, part of #428). Role obligations (you became a requested reviewer,
-- or your authored PR flipped to CHANGES_REQUESTED) now also toast + write an
-- inbox row, deduped per (account, PR) so the emission fires once per obligation
-- episode and re-arms when it clears. This column is the dedup marker; NULL
-- means re-armed (no obligation toasted).
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

-- Last role signature toasted for this (account, PR): 'review_request' |
-- 'changes_requested' | NULL (re-armed / no obligation). Separate from
-- last_emitted_activity_at (the conversation-unit dedup) so a PR can emit both
-- a conversation trigger and a role trigger in one cycle.
ALTER TABLE pull_request_viewer_relations ADD COLUMN last_emitted_role TEXT;
