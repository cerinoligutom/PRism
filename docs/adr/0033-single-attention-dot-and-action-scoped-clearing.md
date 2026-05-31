# 0033 - One attention dot: open-cleared obligations, action-scoped conversation clearing, and the reviews unit

- **Status:** Accepted
- **Date:** 2026-05-31
- **Issue:** [#456](https://github.com/cerinoligutom/PRism/issues/456)
- **Deciders:** @cerinoligutom

## Context

ADR 0031 gave the PR row a two-axis left edge: an **attention dot** bound to the `needs_attention` roll-up, and a **bold title** bound to the PR-level `unread` (open) watermark, plus a my-review-state glyph. In daily use three things proved wrong:

1. **Two marks for one job.** A title that bolds and unbolds next to a dot is redundant, and the row's "Mark unread" overflow action operates on the bold-title axis while the user is reading the dot. The action and the prominent signal don't interact, so it reads as broken (the optimistic-flip comments in `dashboard.ts` even mislabel the `unread` flip as "the dot", a leftover from before 0031 split the dot onto `needs_attention`).
2. **Clearing a conversation is manual-button-only in practice**, which is friction the user wants gone. Opening a PR currently auto-marks every visible unit seen (`auto_mark_units_seen`), which is the opposite extreme - it clears things you never looked at (0031's own "lose-track" failure, flagged in its open follow-ups as "viewport-tracked per-unit seen ... if on-open proves too eager").
3. **An @-mention in a formal review body is invisible.** `mentions_viewer` is scanned only on `review_comments` and `issue_comments` (migration 0025), never on the `reviews` table, so "@you please look" written into a review summary never surfaces.

Separately, the open path (`load_pr_conversation`) advances watermarks and refreshes the dock badge but emits no `dashboard://refresh`, so the in-app sidebar chip and a mounted inbox don't reconcile until the next sync cycle. It stays silent because the conversation store re-invokes `load_pr_conversation` for every visible PR on that same event, so emitting it there would re-enter (loop).

This ADR amends 0031. It does not touch 0031's core (per-conversation-unit watermark, derived inbox, edge-with-re-arm dispatch); it changes how many signals the row carries, what clears an obligation, adds the reviews unit, and adds low-friction auto-seen.

## Decision drivers

- **One signal.** The dot is the attention affordance; a second always-on axis competing with it is the smell.
- **Reduce friction without losing track.** Clearing should follow a genuine interaction (expand a thread, dwell on its tab, arrive via a deep link), not a manual button and not a blunt "opened the PR". An obligation that drops off the dot must remain visible somewhere durable.
- **One roll-up, every surface agrees.** Keep 0031's property that the dot, dock badge, sidebar chip and inbox all derive from the same definition.
- **Reuse before building.** Repurpose the open watermark we're freeing rather than add columns; one new unit that mirrors an existing one, not a new shape.
- **Read-only v1.** Replying / resolving / reviewing happen on GitHub and arrive via sync; in-app clearing is mark-seen.

## Considered options

### How many signals on the row

1. **One dot.** The dot is the sole attention signal; the bold-title axis is removed. Resolves the redundancy and the "Mark unread" disconnect.
2. **Keep the two axes (status quo).** The smell that prompted this.

### What clears a role obligation (review-owed / changes-on-your-PR)

1. **Clear on open; durable state carried by the glyph / MergeableBadge.** The dot is a "new, attend to this" cue; opening satisfies it. The obligation itself stays visible on the my-review-state glyph (review-requested) and the MergeableBadge (CHANGES_REQUESTED) until GitHub state changes.
2. **Clear from GitHub state only (0031 status quo).** Keeps the owed review lit on the dot and badge until submitted; the user finds the persistent dot/badge friction, and the durable indicators now exist to carry it without the dot.
3. **Dot clears on open, badge does not.** Decouples the surfaces; breaks 0031's "one roll-up, agree by construction". Rejected.

### What clears a conversation unit

1. **Reply / resolve (synced from GitHub) plus interaction-driven auto-seen, manual fallback kept.** Auto-seen fires on a deliberate interaction with the unit, not on opening the PR.
2. **Manual mark-seen only.** The friction being removed.
3. **Open clears all (current `auto_mark_units_seen`).** Lose-track.

### Review-body mentions

1. **A peer "reviews" unit** with its own seen watermark and a Reviews-tab "Mark all seen", mirroring the general stream. The mention is the involvement hook.
2. **Fold into the general stream.** The mention shows in the Reviews tab but the clear lives in the Comments tab - a tab mismatch.
3. **Leave unscanned (status quo).** The blind spot.

## Decision

Options 1 across the board.

### One signal

The PR row carries the **attention dot** (the `needs_attention` roll-up) and the my-review-state glyph. The bold-title `unread` rendering is removed; the row "Mark unread" action and the PR-level `mark_pr_read` / `mark_pr_unread` / `mark_view_read` commands retire with it (a bulk "mark all seen" may replace the latter - see follow-ups). The open watermark (`read_at`) is freed from driving the title and repurposed below.

### The dot's sources and how each clears

The roll-up is `1` iff any source is active. Sources split into two classes:

**Conversation-class (survive opening the PR; clear on reply / resolve / seen):**
- **A. Review thread needs you** and **B. general stream needs you** - unchanged from 0031.
- **E. Reviews unit needs you** - a formal review authored by someone else whose body `@`-mentions you, newer than your reviews-seen watermark. Mention-only: the mention is the involvement hook, so a plain Approved/Commented verdict with no mention does not light it (the verdict is the glyph's / MergeableBadge's job).

**Obligation-class (clear on opening the PR; re-arm on a newer obligation event):**
- **C. Review owed** (requested reviewer) and **D. CHANGES_REQUESTED on your authored PR**.

An obligation dot shows iff the obligation is active **and** its onset is newer than the open watermark: `active AND onset > COALESCE(read_at, 0)`. Opening the PR advances `read_at` (it already does), so the dot clears; a later fresh obligation re-arms it, exactly as a fresh reply re-arms a conversation unit - one "newer-than-watermark" rule across every source. Onsets: D = the blocking review's `submitted_at`; C = the latest review-request event time for the viewer (synced timeline, ADR 0027) or a `requested_at` preserved on the requested-reviewer row if the event is unavailable.

**Not lose-track:** an obligation that clears off the dot stays visible on the durable, non-dot indicators 0031 added - the my-review-state glyph reads "review requested" until you submit (C), and the MergeableBadge reads `CHANGES_REQUESTED` until the decision flips (D). The dot is "new / unattended"; the glyph and badge are "your standing relationship". This is the considered re-introduction of the clear-on-open behaviour 0031 reverted: safe now because those indicators carry the obligation, where previously it lived only on the dot/badge.

### Interaction-driven auto-seen (the friction fix)

A conversation unit is marked seen by a deliberate interaction, in addition to reply/resolve and the manual buttons:

- **Thread**: expanding an unread thread card marks that thread seen (fires on expand, not collapse; no-op if already seen). The thread list starts collapsed, so nothing fires until you act.
- **General stream / reviews**: a brief **dwell** (the tab being the active one for ~1s) on the Comments tab marks the general stream seen, and on the Reviews tab marks the reviews unit seen. Dwell, not scroll-to-end, was chosen for simplicity; the accepted cost is that a long stream can clear with a mention still below the fold. Hover is explicitly not a trigger (incidental, no intent).
- **Deep link**: arriving at a thread via a notification deep link (`scrollToPendingThread`) marks that target thread seen.

The manual per-thread "Mark seen" and per-stream "Mark all seen" buttons stay as fallbacks (read it on GitHub, want it gone) but become rarely needed.

### Inbox, badge, chip parity

Every notification row's derived unread mirrors its source's predicate, so the surfaces still agree: obligation rows (`review_request` / `changes_requested`) now also gate on the open watermark (clear on open) to match the dot; a new **`review`** unit kind covers branch E and clears via the reviews-seen watermark. The dock badge and sidebar chip continue to count from the roll-up / unit predicates.

### Live reconcile (the single-seam fix)

Split the conversation command in two:
- `load_pr_conversation` stays the **mutating open** path: it advances the obligation open-watermark (and nothing else now - it no longer auto-marks conversation units seen), then emits `dashboard://refresh` so the row dot, badge, chip and inbox reconcile in the same paint as the open.
- A new **non-mutating reader** serves the conversation store's background sync-cycle re-reads; it does not auto-mark and does not emit, so the re-entrancy loop is gone.

This is what makes "open clears the obligation dot" and "the sidebar chip updates when I open a PR" actually live instead of waiting for the next sync cycle.

### Schema (migration 0028)

```sql
-- Review-body @-mention bit, scanned by the existing word-boundary scanner extended to review bodies.
ALTER TABLE reviews ADD COLUMN mentions_viewer INTEGER NOT NULL DEFAULT 0;

-- Per-PR reviews-stream seen watermark, peer to general_stream_seen_at, on the relation row.
ALTER TABLE pull_request_viewer_relations ADD COLUMN reviews_seen_at INTEGER;

-- If the review-request timeline event is not a reliable onset source, preserve the request time
-- across the per-cycle requested_reviewers wipe-rewrite (set on first insert, kept on re-insert):
-- ALTER TABLE requested_reviewers ADD COLUMN requested_at INTEGER;  -- decided at implementation
```

`read_pr_updated_at` (the bold-title "unread after PR update" snapshot) becomes dead with the unread axis and is left vestigial for a later cleanup ADR (a SQLite column drop is a table rebuild). `read_at` is kept and repurposed as the obligation open-watermark. `0028` is the next free migration number; `migrate.rs` asserts `version == len`, so verify no other migration PR is mid-flight before claiming it.

## Consequences

### Positive

- One attention signal. The dot-vs-bold-title redundancy and the "Mark unread" disconnect are removed by removing the axis, not patched.
- Less friction: conversations clear on the interaction you were already doing (expanding, dwelling, following a deep link), not a manual button.
- Review-body mentions stop being a blind spot; they get a unit, an inbox kind, and a clear path.
- Opening a PR reconciles the dot, badge, chip and inbox live (fixes the sidebar staleness), with no re-entrancy loop.

### Negative

- **Reverses 0031's "owed review stays lit until submitted".** An opened-but-unsubmitted review goes quiet on the dot and the dock badge; the cue moves to the my-review-state glyph and the MergeableBadge, which are less attention-grabbing than the dot. This is the behaviour reverted before, re-introduced deliberately because those durable indicators now carry it and the user prefers lower stickiness.
- Conversation dots get stickier than today: they survive opening and clear only on reply / resolve / a deliberate interaction. The auto-seen triggers are what keep that from becoming a manual chore.
- Dwell (not scroll-to-end) on the single-unit tabs can mark a stream seen with an unread mention still below the fold.
- More per-cycle work: a fifth source and the onset comparisons for obligations.

### Neutral / follow-ups

- A bulk "mark all seen" to replace the retired `mark_view_read`, if a sweep-clear is still wanted.
- Finalise the C onset source (timeline event vs preserved `requested_at`) during implementation.
- Tune the dwell duration; revisit scroll-to-end if dwell proves too eager on long streams.
- A cleanup ADR to drop `read_pr_updated_at` and any other unread-axis remnants.

## References

- Amends and supersedes-in-part: ADR [0031](0031-conversation-unit-attention-and-rearm-dispatch.md) - its "Left-edge encoding" (the bold-title axis) and "Role obligations ... clear from GitHub state, never from mark-seen". Resolves 0031's open follow-up on eager on-open marking.
- Relates to the earlier clear-on-open badge behaviour that was reverted (PR #444 kept the unread axis distinct, clearing on open); this ADR removes that axis and re-introduces clear-on-open for obligations with the glyph/badge as the durable carrier.
- Migration: `src-tauri/migrations/0028_*.sql` (reviews mention bit + reviews-seen watermark).
- Builds on: ADR [0027](0027-timeline-event-expansion.md) (review-request onset), ADR [0016](0016-unified-multi-account-dashboard.md) (host isolation), ADR [0021](0021-rust-to-typescript-type-bindings.md) (`my_review_state` DTO mirrored by hand).
