# 0018 - Archive bucket: per-(account, PR) `archived_at`, 30-day inactivity TTL, manual + auto, reversible

- **Status:** Accepted
- **Date:** 2026-05-22
- **Issue:** [#189](https://github.com/cerinoligutom/PRism/issues/189)
- **Deciders:** @cerinoligutom

## Context

M6 adds an Archive bucket and an inactivity TTL for closed / merged PRs. The roadmap line is "archive bucket, TTL"; the wiki Architecture page elaborates but contradicts itself:

- Line 64: "30-day inactivity TTL: closed/merged PRs auto-archive after 30 days inactive."
- Line 66: "Closed/merged retention is 14 days by default (configurable) before archive."

Two different windows for the same rule. One must be canonical.

What exists today:

- `pull_requests.state` carries `open`, `closed`, `merged`.
- `pull_request_viewer_relations` already holds per-(account, PR) state: `read_at`, `mentioned_count_unread`, `needs_attention`, `last_seen_at` (ADR 0015).
- The dashboard query (`src-tauri/src/dashboard/query.rs:494`) reads from `pull_requests` with LEFT JOINs through repos / relations.
- No `archived_at` column anywhere; no archive view; no row affordance.

Six sub-decisions need pinning:

1. **Column placement** - per-(account, PR) or global-per-PR?
2. **Auto-archive condition** - which PRs auto-archive, and at what age threshold?
3. **Manual archive UX** - row overflow menu? Drawer button? Both?
4. **Reversibility** - is unarchive supported, and from where?
5. **Dashboard query exclusion** - how do default views hide archived rows, and what does the Archive view look like?
6. **Wiki contradiction** - 30 days or 14 days? Configurable?

## Decision drivers

- **Per-viewer semantics.** Read-state, mention counters, and attention are per-(account, PR) per ADR 0015. A user with two accounts may want to archive a PR through one identity while the other identity still cares.
- **Reuse ADR 0015's pattern.** The same table holds the same shape of state; adding `archived_at` keeps the cascade chain and the lifecycle alignment.
- **Idempotent under sync re-runs.** The sweep runs every cycle; running it twice must produce the same result as running it once. NULL -> not-NULL is idempotent; NULL stays NULL once set.
- **Bounded query cost.** The dashboard reads a few hundred PRs at most. Adding `AND rel.archived_at IS NULL` to the existing LEFT JOIN predicate is free.
- **Manual + auto with shared mechanism.** "Archive" should mean the same thing whether the user clicked it or the sweep set it. One column, two writers.
- **Reversibility is cheap and surprises users when missing.** "Oops, I clicked the wrong thing" is a recurring gripe; an `Unarchive` is trivial when `archived_at` is just a timestamp.
- **Honour the roadmap's plain reading.** The roadmap line says "30-day inactivity TTL". The 14-day figure in Architecture line 66 looks like a stale early draft. Pinning 30 resolves the contradiction without re-opening scope.

## Considered options

### Column placement

1. **`archived_at INTEGER NULL` on `pull_request_viewer_relations`.** Per-(account, PR). Mirrors ADR 0015's read-state shape. Multi-account: each viewer archives independently.
2. **`archived_at INTEGER NULL` on `pull_requests`.** Global-per-PR. Cheaper to query (no relation row required). Wrong shape for multi-account: archiving on one account hides the PR on every account that sees it.
3. **New `pr_archive_state` table keyed `(account_id, pull_request_id)`.** Single-purpose, clean. Costs a new ON DELETE CASCADE chain and a new index for a single nullable column - more machinery than the value justifies.
4. **In-memory only.** Resets on restart, doesn't survive sync re-runs. Defeats the whole purpose.

### Auto-archive condition

1. **`state IN ('closed', 'merged') AND pr.updated_at < now - 30 days`** for every relation. Single rule; mirrors the roadmap line.
2. **Closed: 30 days; merged: 14 days.** Two-tier (merged is "done", closed is "abandoned"). Conceptually defensible but adds a knob with no user signal that says it's needed.
3. **Open PRs included after 60 days of inactivity.** Aggressive; risks hiding the slow-moving but-still-active long-running PR.
4. **Configurable threshold in `app_settings`.** Power-user knob; adds a setting nobody asks to tune.

### Manual archive UX

1. **Row overflow menu (`...` icon on hover) with "Archive" / "Unarchive".** Matches ADR 0015's planned "Mark unread" affordance (which lives in the same overflow). One discoverable surface.
2. **Drawer button + row overflow.** Both. Adds redundancy; drawer real-estate is at a premium.
3. **Keyboard shortcut only (`E` for archive).** Powerful but undiscoverable. Could ride along with option 1 in M7 polish, but not a primary surface.
4. **Bulk action (multi-select)** - over-engineered for v1; v1 doesn't have multi-select anywhere yet.

### Reversibility

1. **Unarchive available from the Archive view's row overflow.** Set `archived_at = NULL`. Same column, opposite write.
2. **One-way archive; deletion-after-90-days.** Closer to GitHub Notifications' "Done" semantics but loses the comment history we paid to fetch.
3. **No archive view, no unarchive.** Auto-archived rows become invisible; users tolerate it. Probably defensible but hostile to "wait, where did that PR go?" moments.

### Dashboard query exclusion

1. **Default views add `AND rel.archived_at IS NULL`.** Authored / Assigned / Watching / Team all gain one predicate. Archive view inverts: `AND rel.archived_at IS NOT NULL`.
2. **Archived rows shown with reduced opacity in default views.** "Soft-archived" - visible but de-emphasised. Visual clutter; users will ask "why is this still here".
3. **Quick-filter chip "Show archived".** Per-view toggle. Adds chip-bar real-estate; not the dominant flow.

### Wiki contradiction resolution

1. **30 days is canonical.** Matches the roadmap and Architecture line 64. Remove line 66 entirely.
2. **14 days is canonical.** Aligns with line 66; reopens the roadmap line.
3. **30 days default + configurable.** Both lines true if the default is 30 and there's a knob. Over-engineered for v1.

## Decision

### Column placement - option 1 (`archived_at` on `pull_request_viewer_relations`)

```sql
ALTER TABLE pull_request_viewer_relations
ADD COLUMN archived_at INTEGER NULL;

CREATE INDEX IF NOT EXISTS idx_relations_archived_at
    ON pull_request_viewer_relations (archived_at)
    WHERE archived_at IS NOT NULL;
```

Partial index: only the not-NULL subset is indexed - smaller index, faster sweep queries, no index-maintenance cost for the common case (most rows are unarchived).

Per-(account, PR) means the Archive view is per-account-scope-aware. In unified scope (`accountFilter = null`), the merged-row pattern from ADR 0016 carries through: a PR is archived in the unified view iff every relation owner has archived it (`MIN(archived_at IS NULL) = 0`). A PR archived from one account but still active in another stays visible in unified scope, dotted with that account's marker.

### Auto-archive condition - option 1

```sql
UPDATE pull_request_viewer_relations
SET archived_at = strftime('%s', 'now')
WHERE archived_at IS NULL
  AND pull_request_id IN (
      SELECT id FROM pull_requests
      WHERE state IN ('closed', 'merged')
        AND updated_at < strftime('%s', 'now', '-30 days')
  );
```

Runs once per sync cycle after the per-account fan-out completes (one global sweep, not per-account, since the predicate depends on `pull_requests.state` which is account-agnostic). Idempotent: rows already archived stay archived; the predicate skips them.

The 30-day clock measures inactivity (`pull_requests.updated_at`), not state-transition time. A PR closed yesterday but with a comment 5 days ago has `updated_at` 5 days old, not 1; the sweep waits the full 30. This matches the roadmap's "inactivity TTL" framing and avoids the surprise where a re-opened PR's archive clock restarts on the close event.

### Manual archive UX - option 1 (row overflow)

The PR row's overflow menu (the `...` icon that already houses ADR 0015's planned "Mark unread") gains two entries:

- **Archive** - visible when `archived_at IS NULL` for at least one in-scope relation.
- **Unarchive** - visible when `archived_at IS NOT NULL` for all in-scope relations.

Write semantics under unified scope (`accountFilter = null`): the action applies to every relation owner the viewer holds, each write independent (mirrors ADR 0016's mark-read semantics). Optimistic UI flip; reload from the canonical state on next sync.

### Reversibility - option 1 (Unarchive from Archive view)

Unarchive sets `archived_at = NULL`. Reachable from:
- The Archive view's row overflow.
- A row in any default view that became unarchived between renders (e.g. via a separate device or a database-level fix) - the row reappears in the default views without further user action.

Unarchive in unified scope: same all-relations write as Archive.

### Dashboard query exclusion - option 1

```rust
// src-tauri/src/dashboard/query.rs - default-view predicate
AND rel.archived_at IS NULL
```

Adds one predicate to each of the four default-view subqueries. The Archive view is a new `DashboardView::Archive` variant whose subquery inverts: `AND rel.archived_at IS NOT NULL`, joined across all relation owners with no `state` filter (archive holds closed, merged, and any user-archived-while-open PRs).

The four default views' sidebar attention counts (`list_sidebar_attention_counts` in `triage/query.rs`) gain the same predicate: archived PRs do not contribute to attention totals.

### Wiki contradiction resolution - option 1 (30 days, canonical)

The PR that implements ADR 0018 rewrites `docs/wiki/Architecture.md:64-66` to a single line:

> 30-day inactivity TTL: closed/merged PRs auto-archive after 30 days inactive. Open PRs go stale visually after 30 days but stay visible behind a "Stale" filter chip. Archive is reversible (Unarchive from the Archive view).

The CONTRIBUTING wiki-sync block flags the page for republishing.

## Consequences

### Positive

- Per-(account, PR) archive cleanly matches the read-state pattern; multi-account users get the right behaviour without a second decision in the SQL.
- Manual + auto share one column. Sweep and overflow click write the same `archived_at`. One column, one set of dashboard predicates, no duplicate paths.
- Reversibility is one column-write away. No special unarchive table or state machine.
- Wiki contradiction is resolved without reopening the roadmap.

### Negative

- Per-(account, PR) means the sweep updates `N x accounts` rows instead of `N` rows. At v1 scale (few hundred PRs, low single digits of accounts) this is negligible; the partial index keeps the predicate cheap.
- The merged-row unified-scope semantics (archived iff every relation is archived) means a PR archived from one account but still active on another stays in the dashboard with a "1 of 2 accounts archived" hint. UX hint TBD in implementation; the data model is correct.
- Closed retention is now 30 days flat; users who want shorter retention (e.g. for a "Done" feel) have no knob. Acceptable v1 trade-off; can become configurable post-v1 if a real signal arrives.

### Open follow-ups (not blocking M6)

- Post-v1: configurable retention window if power users ask.
- Post-v1: bulk archive action (multi-select).
- Post-v1: keyboard shortcut `E` for archive.
- Post-v1: "Archive policy" preset (aggressive / standard / lenient).
- Post-v1: optional permanent delete of archived rows older than 90 days to bound DB size.

## Wiki sync

The Architecture page's [Auto-tracking section](../wiki/Architecture.md) currently has contradictory lines 64 and 66. The implementation PR collapses both into one canonical sentence (see "Wiki contradiction resolution" above) and flags the wiki for republish in CONTRIBUTING.
