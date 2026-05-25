# 0028 - Persistent notifications inbox: data model, dispatch, read/unread, retention

- **Status:** Accepted
- **Date:** 2026-05-25
- **Issue:** [#381](https://github.com/cerinoligutom/PRism/issues/381)
- **Deciders:** @cerinoligutom

## Context

ADR 0017 (`docs/adr/0017-desktop-notifications.md`) pinned the OS-toast pipeline: `recompute_needs_attention` detects the two trigger transitions (`needs_attention` 0->1 flip, `mentioned_count_unread` increment), the `notification_sink` trait routes them, and `TauriNotificationSink` in `src-tauri/src/notify/runtime.rs` fires the native toast. `PendingPayloadQueue` stages the click payload so the frontend can route to the right PR.

Nothing is persisted. The toast is ephemeral by design - if the user has focus elsewhere, has the OS notification centre cleared, or isn't at the machine when it fires, the signal is gone. There's no in-app history, no way to ask "what did I miss since lunch", and no recovery if the OS-level notification was dismissed before the user read it.

This ADR captures the move from ephemeral toasts to a persisted inbox with read/unread, retention, and a sidebar entry. The OS-toast pipeline from ADR 0017 stays intact - the inbox is an additional sink hanging off the same dispatch point, not a replacement.

Five sub-decisions need pinning before the three implementation slices (#378 foundation, #379 read/unread, #380 retention) fan out:

1. **Persistence shape.** What does a notification row look like, and how does it survive PR pruning?
2. **Retention.** How do we bound DB growth - count cap, date TTL, or both?
3. **Read/unread model.** One column, separate table, or no read state at all?
4. **Deep-link click contract.** Where does a row click land the user when the source PR may no longer be in the local DB?
5. **Sidebar placement.** Where does the inbox entry live, given PRism has no global header?

## Decision drivers

- **One dispatch path.** The "which transition fires a toast" decision already happens once in `recompute_needs_attention` -> `TauriNotificationSink`. Reproducing that logic for the inbox would invite drift between the two surfaces. Whatever the inbox does, it has to be driven from the same call site.
- **Snapshot survives PR pruning.** Archive TTL (ADR 0018) and account removal both prune `pull_requests` rows. A notification row whose deep-link breaks the moment its source PR is archived is worse than no inbox - it accumulates dead-end entries the user has to clear manually. The row needs to carry enough context to render and link out on its own.
- **Bounded DB growth without UX surprise.** Notifications are a higher-velocity record than PRs - one trigger per PR transition, accumulating across all in-scope accounts. Unbounded growth is a real failure mode at v1 scale (months of use, multiple accounts).
- **Signal what's new since last opened, without becoming annoying.** The whole value of an in-app inbox over OS notification history is telling the user what landed while they were away. That requires read/unread. It does not require a dock badge, an auto-popup, or a separate "ping" - those are toast concerns.
- **Mirror existing patterns.** Archive (ADR 0018) is the closest precedent: a sidebar entry, a dedicated view, a configurable retention knob, and a SQL-driven sweep. Settings UI muscle-memory is real; the retention setting should feel like a sibling of `auto_archive_days`.
- **Trigger set stays.** Expanding from the two ADR 0017 triggers (`needs_attention`, `mention`) to a wider taxonomy is a separate scope question. This ADR pins what to do with the existing set, not which set.

## Considered options

### Persistence shape

1. **New `notifications` table with self-contained snapshot columns.** `owner`, `repo`, `pr_number`, `pr_node_id`, `pr_title`, `title`, `body`, `created_at`, `kind`, `account_id`, plus a nullable FK `pull_request_id`. Row renders and deep-links even after the source PR is pruned.
2. **FK-only reference to `pull_requests`.** `notifications (id, pr_id, kind, created_at)`; everything else joined on read. Cheap to write; broken display once `pr_id` is pruned.
3. **JSON log file outside SQLite.** Append-only NDJSON in app-data dir. No migration needed; awkward to query, awkward to write transactionally with the dispatch.

### Retention

1. **Count cap, configurable, default 500.** `app_settings.notification_retention_max`. Pruned on insert. Every user gets the same "last N" experience regardless of activity.
2. **Date TTL, mirroring `auto_archive_days`.** `notification_retention_days`, sweep on sync cycle. Active users lose recent context if too short; quiet users see ancient entries if too long.
3. **Both (count cap and date TTL).** Two knobs. More UI surface; users tune one and forget the other.
4. **Neither (unbounded).** Simplest; defers the problem to a v1.x patch when someone's table hits 50,000 rows.

### Read/unread model

1. **Single `read_at: i64?` column** on `notifications`. Nullable unix-seconds timestamp. NULL = unread. Indexed via partial index on `WHERE read_at IS NULL`. Mark-on-click; "Mark all as read" action; subtle unread chip on sidebar.
2. **Separate `notification_states` table** keyed `(notification_id, ...)`. More normalised; the join would dwarf the read win for a column that's nullable on 95% of rows after a week.
3. **Boolean `is_read`.** Simpler write semantics; loses the "when was it read" signal that the timestamp variant carries for free.
4. **No read state.** Every row visually equivalent. Inbox becomes a flat log; can't answer "what's new since last open".

### Deep-link click contract

1. **3-state fallback.** State A: PR exists in local `pull_requests` -> open the existing PR drawer. State B: PR not in local DB but snapshot has `owner/repo/pr_number` -> open `https://github.com/{owner}/{repo}/pull/{pr_number}` externally via `@tauri-apps/plugin-opener`. State C: external open fails -> "PR no longer available" toast.
2. **Strict-only.** Open the PR drawer if the PR is in the local DB; otherwise grey out the row. Loses the link the moment Archive TTL runs.
3. **Always-external.** Every click opens GitHub in the browser. Avoids the State A/B branch but throws away the in-app drawer for rows where it would work.

### Sidebar placement

1. **Above Settings with a visual separator** between the PR-view group (Inbox / Review requested / Created / Archive) and the meta group (Notifications / Settings). Notifications sits with the meta group.
2. **Alongside the PR views.** Notifications joins Inbox / Review requested / Created / Archive as a fifth row.
3. **Top-bar bell + dropdown.** Header chrome instead of a sidebar entry. PRism has no global header today; this ADR would have to invent one.

## Decision

### Persistence shape - option 1 (new table with snapshot columns)

```sql
CREATE TABLE notifications (
    id               INTEGER PRIMARY KEY,
    kind             TEXT    NOT NULL,  -- 'needs_attention' | 'mention'
    account_id       INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pull_request_id  INTEGER          REFERENCES pull_requests(id) ON DELETE SET NULL,
    owner            TEXT    NOT NULL,
    repo             TEXT    NOT NULL,
    pr_number        INTEGER NOT NULL,
    pr_node_id       TEXT    NOT NULL,
    pr_title         TEXT    NOT NULL,
    title            TEXT    NOT NULL,
    body             TEXT,
    created_at       INTEGER NOT NULL
);
CREATE INDEX idx_notifications_created_at ON notifications (created_at DESC);
```

The FK to `pull_requests` is nullable with `ON DELETE SET NULL` so that pruning a PR row (Archive TTL, account removal) clears the back-reference without destroying the notification. The snapshot columns (`owner`, `repo`, `pr_number`, `pr_node_id`, `pr_title`, `title`, `body`) carry enough to render the row and reconstruct the GitHub URL when the local PR is gone. `account_id` cascades on account deletion - if the user removes the account, their notifications go with it.

Rationale: option 2 (FK-only) optimises for write cost at the expense of display correctness after pruning. The data is small (a few hundred bytes per row, capped at 500 rows by the retention decision), so the duplicate-storage cost is negligible. Option 3 (JSON log) couples the inbox to a separate persistence layer that doesn't share the SQLite transaction with the dispatch, and the inbox view would need to parse on every load.

### Retention - option 1 (count cap, default 500)

```sql
-- 0023_notification_retention.sql
ALTER TABLE app_settings
ADD COLUMN notification_retention_max INTEGER NOT NULL DEFAULT 500;

-- prune on insert (executed alongside the dispatch INSERT):
DELETE FROM notifications
WHERE id NOT IN (
    SELECT id FROM notifications
    ORDER BY created_at DESC, id DESC
    LIMIT ?  -- bound from app_settings.notification_retention_max
);
```

Recommend min 50 / max 5000 in the settings UI. The setting lives next to the existing notification toggles in `src/views/settings/NotificationsSettings.vue`, not in `SyncSettings.vue`.

Rationale: a date TTL behaves badly at both ends of the activity spectrum. A 30-day window drops recent context for an active user mid-review-cycle; a 90-day window leaves a quiet user staring at notifications from a project they finished two months ago. A count cap gives every user the same "most recent N" experience independent of velocity, prunes cheaply on insert, and keeps DB size predictable to within a known bound (500 rows x ~500 bytes ~= 250 KB).

`auto_archive_days` (ADR 0018) was considered as a muscle-memory parallel - same column shape, same settings placement, same SQL sweep pattern. It was rejected because Archive operates on PR-state-transition records (low volume per PR, high half-life) while notifications operate on trigger events (high volume, low half-life). A count discipline fits the higher-velocity record.

Both knobs (option 3) was rejected on UI-surface grounds: two settings that mostly do the same job invite users to tune one and forget the other.

### Read/unread model - option 1 (single `read_at` column)

```sql
-- 0022_notifications_read_at.sql
ALTER TABLE notifications ADD COLUMN read_at INTEGER NULL;

CREATE INDEX idx_notifications_unread
    ON notifications (created_at DESC)
    WHERE read_at IS NULL;
```

Behaviour:

- Mark-on-click. Clicking the row optimistically sets `read_at = strftime('%s', 'now')` (if NULL), then dispatches the deep-link.
- `Mark all as read` action in the view header sets `read_at` on all unread rows. Only enabled when `unreadCount > 0`.
- Sidebar shows a subtle unread count chip on the Notifications entry when `unreadCount > 0`. No chip when zero.
- No dock badge, no auto-popup, no toast on unread arrival. The OS toast (ADR 0017) is the only "attention right now" surface; the unread chip is the passive "since you last looked" surface.

Rationale: option 2 (separate states table) is the textbook normalised choice and the wrong one here - it adds a join to every list query for a column that's nullable on every row by default and toggles once per row's lifetime. Option 3 (boolean) gives up the "when was it read" timestamp for no implementation win; the column is the same size either way. Option 4 (no read state) defeats the inbox's only advantage over scrolling back through the OS notification centre.

### Deep-link click - option 1 (3-state fallback)

Click handler logic:

1. **State A.** Query `pull_requests` by `pr_node_id` (preferred) or `(owner, repo, pr_number)`. If a row exists in the local DB and is visible under the current `accountFilter`, open the existing PR drawer via the same flow as `PullRequestRow.vue`.
2. **State B.** Local DB miss but the snapshot columns are populated (they always are for new-format rows). Open `https://github.com/{owner}/{repo}/pull/{pr_number}` externally via `@tauri-apps/plugin-opener`.
3. **State C.** External open fails (no network, GitHub itself returned 404 if PRism could detect it, the opener plugin raised an error). Surface "This PR is no longer available" via the existing toast / inline-message pattern.

Rationale: PR rows get pruned. Archive TTL (ADR 0018) sweeps closed/merged PRs at 30 days; account removal cascades to `pull_requests`. The inbox row outlives both, so the click handler has to expect the local row to be gone. State B handles the common case where the PR still exists on GitHub but PRism has dropped its local copy. State C acknowledges that GitHub itself may have lost the PR (deleted repo, deleted account, GHE instance offline) - a clear message beats a silent failure that leaves the user wondering whether they clicked.

Option 2 (strict-only) was rejected because it turns archived-then-archived-from-cache PRs into dead rows the user has to dismiss manually. Option 3 (always-external) was rejected because it gives up the in-app drawer experience for the case where it would work (the typical case: recent notification on a still-open PR).

### Sidebar placement - option 1 (above Settings, with separator)

Sidebar order from top:

```
PR views:
  Inbox
  Review requested
  Created
  Archive
---  (visual separator)
Meta:
  Notifications
  Settings
```

The separator is a thin divider line styled with `border-border-faint`, matching the existing primitive vocabulary. Notifications sits with Settings because both are meta over the PR-view work, not another PR view.

Rationale: option 2 (alongside PR views) would conflate primary work surfaces with the event log. The four PR views are mutually exclusive ways of slicing the same dataset; Notifications is orthogonal - a history of events that fired against any of those views. Grouping it with the PR views misleads the user about what's in there. Option 3 (top-bar bell + dropdown) requires inventing a global header that PRism doesn't have today; the dropdown then becomes its own UX problem (focus management, click-outside dismissal, mobile-window resize) for a surface a sidebar entry handles for free.

### Dispatch path - single insert at the existing sink

The `notifications` row is inserted at the same call site in `src-tauri/src/notify/runtime.rs` where `TauriNotificationSink` fires the OS toast. One dispatch -> two effects (toast + insert + prune). The two surfaces stay in sync by construction, not by convention.

This is not a separate sub-decision (no genuine alternatives were considered) but worth recording as the consequence of the "one dispatch path" decision driver: any future sink-side change (new trigger, new payload shape, new pref check) lands in one file and both surfaces inherit it.

### Trigger set - unchanged from ADR 0017

The two existing triggers (`needs_attention` 0->1 flip, `mentioned_count_unread` increment) populate the inbox. Adding triggers (review submitted, CI failure, request changes, etc.) is explicitly out of scope and tracked as a post-v1 follow-up below. The inbox can absorb new triggers later without a schema change - `kind` is a TEXT column.

## Consequences

### Positive

- One dispatch path means the toast and the inbox can never drift. A change to "which transition fires" lands in one file.
- Snapshot columns preserve readability and deep-linkability after PR pruning. Archive sweeps and account removals don't leave dead inbox rows.
- Count cap bounds DB growth predictably: 500 rows x ~500 bytes ~= 250 KB max. Pruning runs on insert (cheap) instead of on a separate sweep schedule.
- Single `read_at` column carries "is it read" and "when was it read" in one nullable timestamp. Indexed via a partial index so unread queries stay cheap as the table grows toward the cap.
- 3-state fallback acknowledges the real lifecycle (local-cache, GitHub, gone) without papering over the gone-from-everywhere case.
- Sidebar placement signals "this is the event log, not a PR view" through visual grouping rather than through a label.

### Negative

- Extra DB write per notification (one row per toast) plus a prune query. Cheap at v1 scale; not free.
- Read/unread state adds a column and a write path. Optimistic UI on row click is one more thing that can desync from the DB if a write fails - acceptable for a non-critical surface.
- Per-account retention overrides are not in v1. A user with one noisy work account and one quiet personal account shares a global cap. Post-v1 follow-up if a real signal arrives.
- The unread chip on the sidebar is an additional always-on surface. If the user runs PRism in the background for weeks, the chip may grow large enough to look alarming. Bounding chip rendering (e.g. "99+") is a UI polish item, not an architecture item.
- The 3-state fallback's State C path depends on the opener plugin's error surfacing being legible. If `@tauri-apps/plugin-opener` returns a generic "open failed" without distinguishing "URL invalid" from "no network", State C may fire when the user needs to reconnect.

### Open follow-ups (post-v1)

- Trigger set expansion: review submitted, CI failure, request changes, draft -> ready transitions. Each gets a new `kind` value and a new transition detector in or around `recompute_needs_attention`.
- Inbox filter UI: "show unread only" toggle. Cheap to add given the partial index already covers it.
- Per-account retention overrides if a real signal arrives.
- Quiet-hours integration (currently descoped from ADR 0017): when quiet hours suppress the OS toast, the inbox row still gets written. Decide whether the unread chip respects quiet hours or not.
- Bulk dismiss (multi-select) - not part of v1's dismiss/clear-all surface.

## References

- ADR 0017 - Desktop notifications (the toast pipeline this ADR builds on).
- ADR 0018 - Archive bucket and TTL (precedent for the configurable retention setting; rejected for notifications in favour of a count cap).
- ADR 0021 - Rust to TypeScript type bindings (the `Notification` DTO is mirrored by hand under this convention).
- Implementation slices:
  - [#378](https://github.com/cerinoligutom/PRism/issues/378) - foundation slice (table, dispatch wiring, list/delete/clear commands, sidebar entry, click fallback).
  - [#379](https://github.com/cerinoligutom/PRism/issues/379) - read/unread slice (`read_at` column, mark-as-read commands, sidebar unread chip, "Mark all as read").
  - [#380](https://github.com/cerinoligutom/PRism/issues/380) - retention slice (`notification_retention_max` setting, count-cap pruning, settings UI, inbox footer hint).
