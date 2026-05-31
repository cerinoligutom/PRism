# 0031 - Conversation-unit attention model, derived inbox read-state, and edge-with-re-arm dispatch

- **Status:** Accepted
- **Date:** 2026-05-31
- **Issue:** [#428](https://github.com/cerinoligutom/PRism/issues/428)
- **Deciders:** @cerinoligutom

## Context

PRism grew three read-state systems in isolation, and they never shared a definition of "read":

1. **Triage** (ADR 0015, migration 0010): `pull_request_viewer_relations` columns (`read_at`, `read_pr_updated_at`, `mentioned_count_unread`, `mention_scan_watermark_at`, `needs_attention`), keyed `(account_id, pull_request_id)`. Drives the PR-row dot/tint, the sidebar per-view attention dots, and the macOS dock badge.
2. **Notifications inbox** (ADR 0028, migrations 0021-0023): the `notifications` table with its own `read_at`. Drives the sidebar count chip and the list. Rows are inserted append-once, only at OS-toast dispatch time.
3. **OS toasts + badge** (ADR 0017): ephemeral toasts, edge-triggered on a `needs_attention` 0->1 flip or a strict `mentioned_count_unread` increase.

The only seam between them runs one direction (triage to inbox/toast) and is edge-gated, so nothing flows back. A code-level audit confirmed five user-reported symptoms: marking a PR read never marks its inbox row read; there is no per-comment read-state (read is PR-level only); the inbox is sparse because emission fires once per PR; a toast click routes but reconciles neither the inbox row nor the badge (the payload carries only `account_id` + `pull_request_id`); and the badge (a `COUNT` of unread PRs via the `read_at` predicate, `badge.rs`) and the sidebar chip (a `COUNT` of `notifications WHERE read_at IS NULL`) are different queries over different tables that cannot agree by construction.

The audit also surfaced two latent bugs: the `notify_on_needs_attention` / `notify_on_mention` settings toggles are persisted and bound in the UI but never read by `decide_dispatch` (dead controls), and the host-blind involvement test (`a.id IN (SELECT id FROM accounts)`) appears not only in the two `needs_attention` formulas but also in `conversation/query.rs` (`thread_buckets`, `list_pr_threads`), so a multi-account viewer with the same login on two hosts can see cross-host leakage.

The deeper modelling gap: ADR 0015's `needs_attention` signal 1 ("an unresolved thread you are involved in") is gated to PRs the viewer **authored**, so a non-author participant in a review thread goes dark the moment their mention is read - even when the conversation they are part of keeps moving. PRism's whole value proposition is not missing the conversations that involve you, and GitHub itself does not surface "a thread I am in moved since I last engaged" as a durable, per-user signal (subscription is whole-PR and ephemeral). PRism already stores full conversation depth (ADR 0010 / 0029), so it can compute this from data on disk.

This ADR unifies the three systems onto one source of truth. It supersedes in part ADR 0015 (the attention composite), ADR 0017 (the badge/toast definition), and ADR 0028 (the inbox read-state).

## Decision drivers

- **One definition of "read", one source of truth.** The five symptoms are all the same bug: three stores with no shared notion of read. Whatever the model is, every surface must derive from it rather than keep its own copy.
- **Conversation-grained, because that is the unit of attention.** Users track "which conversation needs me", not "which PR changed". The model must point at the exact thread and survive being read for unrelated reasons.
- **Do not lose track.** A signal that clears merely because the PR was opened recreates the problem it solves. Clearing must be a deliberate act (mark-seen) or a genuine engagement (you replied).
- **Live without spam.** The inbox should re-surface a conversation when it genuinely moves, but a level-triggered "one row per PR per cycle" feed would be muted instantly.
- **Reuse before building; MVP over completeness.** Favour columns on the existing relation row over new tables; derive over duplicate; no abstraction until a pattern repeats (CLAUDE.md).
- **Read-only v1.** PRism observes; all writes happen on GitHub. "Reply-driven" clearing is a read of synced GitHub state, not a write.

## Considered options

### Read-state granularity

1. **Per-conversation-unit watermark.** A unit is a review thread or the PR's general comment stream. Read state lives at the unit. Points at the exact conversation; matches the mental model.
2. **PR-level only (status quo).** Simplest, but "which thread?" is unanswerable and opening a PR for one thread clears all of them - the lose-track failure.
3. **Per-comment read-state.** Finest grain. Heavy: a row per comment per viewer, and the value over per-unit is marginal in a read-only observer.

### What clears a unit

1. **Explicit mark-seen plus reply-driven.** A unit clears when the viewer marks it seen, or when the viewer's own later comment (synced from GitHub) advances the watermark. New activity past the watermark re-arms it.
2. **Open-the-PR clears everything.** Blunt; reintroduces lose-track.
3. **Reply-driven only.** No way to dismiss a thread you have decided not to answer.

### Dispatch trigger shape

1. **Edge-with-re-arm.** Keep one dispatch path; emit only when genuinely-new activity crosses a per-relation watermark; reading re-arms for free. Live without spam.
2. **Pure edge (status quo).** Once per attention episode; the sparse inbox.
3. **Level.** A row every cycle a PR needs you; muted instantly.

### Dispatch dedup grain

1. **Per-PR (`last_emitted_activity_at` column on the relation row).** Cheap; cascades and joins for free; the toast deep-links to the PR and the per-thread cue locates the unit. Trade: two units re-lighting in one cycle emit one toast.
2. **Per-unit (new `notification_dispatch_state` table).** Exact per-unit dedup, but a new table whose cardinality grows per-unit-per-PR-per-account, justified only by per-unit toasts. Rejected for v1 under "no abstraction until the pattern repeats"; promotable later.

### Inbox read-state

1. **Derived per row against the row's own unit watermark.** No independent `read_at`; the chip and the badge agree by construction.
2. **Derived against the PR-level roll-up.** Wrong: a PR with one re-lit and one settled unit has `needs_attention = 1`, marking *both* rows unread. Over-counts.
3. **Independent `read_at` (status quo).** The disconnect being removed.

## Decision

### Source of truth: the per-conversation-unit engagement watermark (option 1)

A **conversation unit** is either a review thread (a `review_threads` row) or the PR's **general comment stream** (its `issue_comments`, treated as one dismissible unit per PR). For viewer `v`:

```
last_engaged_at(unit) = MAX(explicit mark-seen timestamp for (v, unit),
                            v's own latest comment.created_at in that unit)
```

A unit **needs `v`** when, host-matched:

- `v` is **involved** in the unit: `v` authored the PR, OR `v` has a comment in the unit, OR a comment in the unit has `mentions_viewer = 1` for `v`; AND
- there exists a comment in the unit **by someone other than `v`** with `created_at > last_engaged_at(unit)`.

Resolved threads need no special branch: a reply newer than the watermark satisfies the predicate, so "resolved + new reply still nags; resolved + quiet stays quiet" falls out for free (the user asked for the nag deliberately - people reply post-resolution to thank or because they forgot to unresolve).

Mentions fold into involvement via a persisted per-comment `mentions_viewer` bit (set by the existing word-boundary scanner). The standalone `mentioned_count_unread` counter is retired as a signal; a mention is just one reason a unit involves you.

### What clears a unit: explicit mark-seen plus reply-driven (option 1)

Two writers advance the watermark: an explicit per-unit "mark seen" action, and the viewer's own later comment arriving via sync (read-only-friendly - replying on GitHub is the natural "I have dealt with this"). Marking seen is **not** mute-forever: a later other-authored comment past the watermark re-lights the unit.

### Role obligations are PR-level and separate

Being a **requested reviewer** (you owe a review) and **CHANGES_REQUESTED on your authored PR** are PR-level obligations, not conversation units. They clear from GitHub state (submitting a review drops you from `requested_reviewers`; addressing changes flips the decision), never from mark-seen. This is what keeps "a review you have read but not yet submitted" lit - the case the old badge-on-`needs_attention` got wrong and was reverted for.

> **Update (2026-05-31)** (issue [#450](https://github.com/cerinoligutom/PRism/issues/450)): role-obligation transitions now also **dispatch**, revising the original "role obligations are badge/sidebar-only, not toasts" stance below. When the viewer newly acquires a role obligation - becomes a requested reviewer, or their authored PR flips to CHANGES_REQUESTED - one toast + inbox entry fires (unit kinds `review_request` / `changes_requested`, `unit_ref` NULL, deep link = the PR conversation URL). This is a SEPARATE per-PR dedup from the conversation `last_emitted_activity_at`, keyed on a new `pull_request_viewer_relations.last_emitted_role` column (migration 0026; `'review_request'` | `'changes_requested'` | NULL where NULL = re-armed). Per (account, PR) the worker computes the current role signature with the SAME host-gated logic as roll-up branches C/D (`'changes_requested'` when CHANGES_REQUESTED on the viewer's authored PR, else `'review_request'` when in `requested_reviewers`, else NULL; CHANGES_REQUESTED wins if both somehow hold): a non-NULL signature differing from `last_emitted_role` emits once and advances the marker; a NULL signature re-arms it. A PR may legitimately emit both a conversation trigger and a role trigger in one cycle (the two dedups are independent). The role inbox row derives unread per-row against the live obligation (the same EXISTS as branches C/D), reading as read once the obligation clears with no `read_at` write. Role dispatch goes through the same `decide_dispatch` master switch + `notify_on_needs_attention` gate and the same sink as conversation toasts. The roll-up / badge / sidebar dots already reflected role obligations via branches C/D; this slice only adds the emission. The remainder of this decision (clearing-from-GitHub-state, the lit-while-unsubmitted property) is unchanged.

### The roll-up: one host-aware definition feeding every surface

`needs_attention` for `(account, pr)` becomes a PR-level **roll-up**: `1` iff any review thread needs me, OR the general stream needs me, OR I am a requested reviewer, OR CHANGES_REQUESTED on my authored PR. The author-only gate on the old signal 1 is removed. This single roll-up feeds the PR-row attention indicator, the dock badge, and the sidebar per-view dots, so the three agree by construction (resolving symptom 5).

The roll-up SQL is consolidated into one host-aware, row-correlated builder used by both callers (the bulk command path and the single-row sync path) - a genuine consolidation, not a copy-paste, because today the command path is row-correlated host-blind and the sync path is param-bound host-aware. The same host gating is applied to `conversation/query.rs` (`thread_buckets`, `list_pr_threads` `is_involved`), because the conversation surface is embedded as ground truth by the "How signals work" page and must not disagree with the row on a multi-account setup.

When there is no relation row (a Tracked/Team PR with no connected viewer identity), the roll-up returns `0` and the conversation surface reports `is_involved = 0` - a unit cannot need you when there is no you.

### Inbox read-state: derived per row against the row's own unit (option 1)

The `notifications` row gains `unit_kind` (`'thread'` | `'general'` | `NULL` for legacy/PR-level), `unit_ref` (the thread `node_id`, or `NULL`), and `deep_link_url`. A live row (with a `pull_request_id`) is **unread iff its referenced unit still needs me**, resolved against that unit's watermark - **not** the PR roll-up. An orphan row (`pull_request_id` set to `NULL` by the existing `ON DELETE SET NULL`) is unread iff `read_at IS NULL`; `read_at` narrows to this orphan-only fallback, and `mark_read` / `mark_all_read` operate only on orphan rows. The chip counts inbox rows whose unit is unread plus unread orphans; the badge counts PRs with any lit unit or obligation; they agree.

### Dispatch: edge-with-re-arm, per-PR dedup, single toggle (options 1 + 1)

One dispatch path (`TauriNotificationSink::dispatch`, reached only by the sync worker). For a PR that currently needs me, emit iff `newest_other_activity_at(pr) > COALESCE(last_emitted_activity_at, 0)`, then advance the column (MAX-only). Reading advances an engagement watermark, the PR stops needing me, nothing fires; a later new reply is both newer than the seen mark (re-lights) and newer than `last_emitted_activity_at` (re-fires exactly once). The collapsed single `notify_on_needs_attention` toggle (the master switch still applies above it) is read in `decide_dispatch`; `notify_on_mention` is retired (left vestigial). The toast click threads `unit_kind` / `unit_ref` / `deep_link_url` so it reconciles the exact unit; with derived read-state, routing + open already clears it.

### Schema (migration 0025)

```sql
-- Per-review-thread explicit "seen" watermark, keyed on the GraphQL node_id (durable
-- across a transient delete+re-add in a paginated fetch). account_id cascades; seen_at advances only.
CREATE TABLE thread_read_state (
    account_id            INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    review_thread_node_id TEXT    NOT NULL,
    seen_at               INTEGER NOT NULL,
    PRIMARY KEY (account_id, review_thread_node_id)
);

-- Per-PR general-stream seen watermark + per-PR dispatch dedup, on the relation row (cascade + join for free).
ALTER TABLE pull_request_viewer_relations ADD COLUMN general_stream_seen_at   INTEGER;
ALTER TABLE pull_request_viewer_relations ADD COLUMN last_emitted_activity_at INTEGER;

-- Inbox unit reference + deep link (per-row derived unread; deep-link the exact unit). All nullable.
ALTER TABLE notifications ADD COLUMN unit_kind     TEXT;  -- 'thread' | 'general' | NULL (legacy)
ALTER TABLE notifications ADD COLUMN unit_ref      TEXT;  -- review_thread_node_id, or NULL
ALTER TABLE notifications ADD COLUMN deep_link_url TEXT;

-- Per-comment mention bit set by the existing scanner; replaces mentioned_count_unread as the signal.
ALTER TABLE review_comments ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;
ALTER TABLE issue_comments  ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;

-- DEPRECATED, left vestigial (a SQLite column drop is a table rebuild for no payoff now):
--   pull_request_viewer_relations.mentioned_count_unread, .mention_scan_watermark_at,
--   settings.notify_on_mention. Stop reading them; a later cleanup ADR may drop them.
```

`0025` is the next free migration number; `migrate.rs` asserts `version == MIGRATION_SOURCES.len()`, so the branch verifies no other migration PR is mid-flight before claiming it.

### Left-edge encoding

The overloaded priority strip (draft > changes-requested > stale > needs-review > approved) is dissolved into two always-visible, orthogonal slots: an **attention dot** bound to the roll-up (the one attention affordance; the separate `pr-row--attention` tint is removed so attention is encoded once), and a **my-review-state** icon (review-requested / approved / changes-requested / commented / author / not-a-reviewer; precedence author > requested > changes > approved > commented > none). Bold-title stays bound to `unread` (a distinct "unopened content" signal). Draft and conflicts move to `MergeableBadge`; stale moves to the time cell; `ReviewerStack` becomes the full per-reviewer confirmation view. `my_review_state` is computed server-side in the roll-up, not derived client-side (the client cannot express "requested but not yet submitted" / "author" / "not a reviewer" from submitted-review data alone).

## Consequences

### Positive

- One source of truth: marking a unit seen flips its inbox row, the chip, and (when it was the PR's last lit unit) the badge together. The five symptoms collapse into this.
- Conversation-grained: notifications point at the exact thread, and "which thread needs me" is answered in the conversation view. The author-only gate is gone, so non-author thread participants stay surfaced.
- Counts agree by construction; the dead toggle and the host-isolation bug are fixed.
- Mostly additive schema on existing rows; one small new table; no destructive migration.

### Negative

- More per-cycle work: the roll-up resolves per-unit watermarks rather than reading one boolean. Bounded by the existing per-PR write overhead.
- Per-PR dispatch dedup is coarser than per-unit: two units re-lighting in one cycle emit one toast (deep-linking to the PR; the per-thread cue locates the unit). Promotable to a per-unit table if it proves too coarse.
- An inbox row whose unit later drops out of "needs me" (you were dropped as a reviewer, the obligation cleared) silently reads as read with no `read_at` timestamp - intended under the derived model, but different from the old explicit-read semantics.
- Vestigial columns (`mentioned_count_unread`, `mention_scan_watermark_at`, `notify_on_mention`) linger until a cleanup ADR.

### Open follow-ups

- Guided spotlight tour on the "How signals work" page - deferred to its own issue + PR for local experimentation; the static page (live components + section anchors) ships first.
- Per-unit dispatch dedup, if per-PR proves too coarse.
- A cleanup ADR to drop the vestigial 0010 columns and the mention toggle.
- Viewport-tracked per-unit "seen" (vs marking all visible units seen on open) if the on-open behaviour proves too eager.

## References

- Supersedes in part: ADR [0015](0015-triage-state-model.md) (attention composite), ADR [0017](0017-desktop-notifications.md) (badge/toast definition), ADR [0028](0028-persistent-notifications-inbox.md) (inbox read-state).
- Builds on: ADR [0010](0010-conversation-depth-storage.md) / [0029](0029-sync-owns-conversation-persistence.md) (conversation depth this model reads), ADR [0016](0016-unified-multi-account-dashboard.md) (host isolation), ADR [0021](0021-rust-to-typescript-type-bindings.md) (the `my_review_state` DTO is mirrored by hand).
- Migration: `src-tauri/migrations/0025_conversation_unit_read_state.sql`; the 2026-05-31 role-dispatch amendment adds `src-tauri/migrations/0026_role_obligation_dispatch.sql` (the `last_emitted_role` dedup column).
- Epic and slice breakdown: [#428](https://github.com/cerinoligutom/PRism/issues/428).
