# 0015 - Triage state model: per-account read-state, mention detection, and "needs my attention" composite

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#144](https://github.com/cerinoligutom/PRism/issues/144)
- **Deciders:** @cerinoligutom

## Context

M2 deferred sorting, filter chips, search, unread indicators, mentions, and the "needs my attention" composite signal to M4. The data scaffolding to compute each signal is now in place: M3 brought the four-bucket threads breakdown (ADR 0012), M2 brought the CI rollup, and #138 brought persisted comment bodies. M4 turns that data into the triage UX promised in the dashboard artboard.

Five sub-decisions need pinning before the parallel Wave-2 / Wave-3 issues can fan out:

1. **Where does per-account read-state live?** A new `pr_read_state` table, columns on the existing `pull_request_viewer_relations` row, or in-memory only?
2. **What counts as "read"?** Opening the detail surface, hovering, an explicit button, or a combination?
3. **What counts as a "mention"?** Verbatim substring, word-boundary regex, parsed markdown? Where does the scan run?
4. **How is the "needs my attention" composite computed and when?** A CTE per query, a precomputed column written by the sync worker, or a frontend derivation?
5. **Does the dashboard search hit the database or stay in-memory?** The dataset is small but the search input is keystroke-frequent.

Each sub-decision below has the same shape: considered options, decision, rationale.

## Decision drivers

- **Per-(account, PR) by definition.** Read-state, mention counters, and attention are all per-viewer. Multi-account viewers see different unread + attention slices across their accounts.
- **Idempotent under sync re-runs.** The sync cycle runs every few minutes and lazy hydration runs on every drawer open; the scanner and the recompute must produce the same state regardless of how many times they fire.
- **One join per dashboard render.** The dashboard query already runs four LEFT JOINs (users, repos, accounts, relations). Adding a CTE-per-render for the attention composite breaks the query plan.
- **Bounded dataset.** Typical v1 accounts surface fewer than a few hundred PRs. Premature search-engine infrastructure is wasted effort.
- **Multi-cycle false-positive tolerance.** Acceptable to miss the occasional mention or count a quoted historical one; not acceptable to thrash the counter every cycle.
- **Reuse existing scaffolding before adding new tables.** `pull_request_viewer_relations` already exists, is keyed `(account_id, pull_request_id)`, and cascades with accounts.

## Considered options

### Read-state storage

1. **New `pr_read_state(account_id, pull_request_id, read_at, read_pr_updated_at, mentioned_count_unread)` table.** Single-purpose table; clean concern boundary. Costs a new ON DELETE CASCADE chain, a new index, a duplicated key with `pull_request_viewer_relations`.
2. **Extend `pull_request_viewer_relations` with the read columns.** Single table per (account, PR); the existing cascade keeps lifecycle aligned with discovery. Mixes read-state with relation-tracking semantics on the same row.
3. **In-memory only (no persistence).** Read-state lives in the frontend Pinia store. Survives until the app restarts and then resets. Doesn't survive multi-device usage either (which v1 doesn't promise, but losing it on restart is jarring).

### Read trigger

1. **Auto-mark on drawer / route open.** Matches the lazy-hydration trigger already in place. Zero new UI affordance.
2. **Explicit "Mark as read" button only.** No auto-mark; user controls the flip. Adds a button to every row; doesn't survive a "I'll read this later" workflow because users forget to click.
3. **Auto-mark on row hover + viewport dwell.** Aggressive; flips PRs the user didn't even glance at properly. False reads erode the unread signal's value.
4. **Auto-mark on drawer / route open PLUS explicit "Mark unread" menu action.** Combines (1) and an inverse of (2). Users get the implicit flow for the 95% case and a manual override for "I want to come back to this later".

### Mention detection

1. **Verbatim substring (`LIKE '%@<login>%'`).** Cheapest. False-positives on substrings: `@alice` matches `@aliceandbob`.
2. **Word-boundary regex (`@<login>(?=[\s.,;:!?)\]'"]|$)`).** Catches the common terminators; misses Unicode-trailing edge cases (acceptable v1 conservatism).
3. **Full markdown parser + AST walk.** Excludes code blocks and blockquotes. Requires a new dependency (`pulldown-cmark` or similar) running on every comment scan. False-positive rate drops to near-zero but cycle CPU cost climbs.
4. **GitHub-server-side detection via the `Notifications` API.** Real mentions per GitHub's own engine. New API surface; requires an additional polling endpoint; doesn't survive notification settings the user has disabled.

### Attention composite

1. **CTE per dashboard query.** `WITH attention AS (SELECT ... FROM ...) SELECT ... FROM pull_requests pr LEFT JOIN attention ...`. Always fresh; expensive on every render and every sidebar count.
2. **Precomputed boolean column on `pull_request_viewer_relations`, written by the sync worker + by `mark_pr_read` / `mark_pr_unread`.** Single column read in the dashboard query; the formula lives in one helper that both writers call. Recomputed at well-defined trigger points.
3. **Frontend-derived from the existing DTO fields.** Compose the composite in TypeScript from `threads_unresolved_involved`, `requested_reviewers`, etc. that the row already carries. Cheapest backend; means the sidebar attention badge can't run a `COUNT(*) WHERE needs_attention = 1` without re-implementing the formula in SQL anyway.

### Search backend

1. **In-memory `String.toLowerCase().includes(query)` over the loaded view rows.** Constant-time per keystroke; bounded by the loaded set (a few hundred rows worst case).
2. **SQLite `LIKE` re-query on every keystroke.** Round-trips through the Rust bridge. Latency dominates on a desktop process boundary.
3. **SQLite FTS5 virtual table over `pull_requests.title + repos.owner + repos.name + author_login`.** Mature substring engine; needs index maintenance + a new dependency feature on rusqlite. Overkill for the dataset size.

## Decision

### Read-state storage: option 2 - extend `pull_request_viewer_relations`

Add `read_at`, `read_pr_updated_at`, `mentioned_count_unread`, `mention_scan_watermark_at`, `needs_attention` columns to the existing table. The relation row already exists for every PR the viewer touches; adding columns is a single migration. The cascade chain from `accounts` keeps lifecycle aligned - dropping an account drops every triage signal for it without a second ON DELETE clause.

The existing `pull_request_viewer_relations.last_seen_at` column is a cycle-bookkeeping timestamp (discovery-phase pruning). The new read watermark uses the `read_at` name to keep the two visibly distinct on the row.

### Read trigger: option 4 - auto-mark on open + explicit "Mark unread"

Opening the drawer / route auto-marks the PR read - same trigger surface as M3's `fetch_pr_conversation` lazy hydration. The conversation hydrator's existing transaction grows one UPDATE against the relation row.

An explicit "Mark unread" menu action lives on the PR row's overflow menu (M4-F polish). An explicit "Mark all read" power-user action is deferred to a separate polish PR; the auto-mark handles the common case so the explicit button isn't on the critical path.

### Mention detection: option 2 - word-boundary regex, scanned at sync time against persisted comment bodies

The sync worker's enrichment phase scans `review_comments.body_text` + `issue_comments.body_text` for new rows whose `created_at > mention_scan_watermark_at` and whose `author_login != accounts.login`. The match pattern is the word-boundary regex above; SQLite's `REGEXP` function (registered via rusqlite's existing custom-function support) handles the per-row evaluation.

Matches increment `mentioned_count_unread` and advance the watermark to `MAX(created_at)` from the scanned set, both inside the same transaction. The watermark prevents double-counting across cycles; the `author_login != accounts.login` check stops the viewer's own comments from counting.

Code-block and blockquote false positives are accepted v1 noise. The full markdown parser was rejected because the noise tolerance is high (a stray `> @alice` quoted from upstream costs a single bump on first encounter and is then watermarked off) and the parser cost runs every cycle.

### Attention composite: option 2 - precomputed `needs_attention` column

A single boolean column on `pull_request_viewer_relations`, written by the sync worker after every cycle and by `mark_pr_read` / `mark_pr_unread` for the mention-driven flip. The composite formula:

```text
needs_attention = 1 IF ANY OF:
  - viewer.login = pull_requests.author_login AND threads_unresolved_involved > 0
  - viewer is in requested_reviewers for this PR (any state including pending)
  - mentioned_count_unread > 0
  - pull_requests.review_decision = 'CHANGES_REQUESTED' AND viewer.login = pull_requests.author_login
```

The partial index `idx_pr_viewer_relations_attention ON pull_request_viewer_relations(account_id) WHERE needs_attention = 1` sizes the sidebar count-badge query: `SELECT COUNT(*) FROM pull_request_viewer_relations WHERE account_id = ? AND needs_attention = 1` becomes a count over the partial index.

Stale-only (older than 7 days but otherwise fine) and CI-failing-alone are explicitly _not_ in the composite. Each has its own filter chip; rolling them into "needs my attention" would dilute the signal.

### Search backend: option 1 - in-memory over loaded rows

The frontend store filters `pullRequests.value` on `title`, `repo.owner/name`, and `author_login`, case-insensitive `.includes()`. Debounced at 150ms. `cmd+K` focuses the input.

If multi-hundred-PR accounts grow into thousands, a fresh ADR can promote this to FTS5 without breaking the contract - the search input + store interface are unchanged.

## Consequences

### Positive

- **Single source of triage truth.** Every signal lives on the same row keyed `(account_id, pull_request_id)`. Multi-account viewers see independent state per account.
- **Cycle-time idempotent.** The mention scanner advances a watermark inside the same transaction as the counter update. Re-runs (including the lazy hydrator's re-writes by `node_id`) never double-count.
- **One-column read for `needs_attention`.** The dashboard query gains one column projection, not a CTE. The sidebar badge becomes a partial-index count.
- **In-memory search keeps the dashboard responsive.** No round-trip per keystroke; no FTS5 dependency in v1.
- **Backward-compatible widening.** `DashboardSort` grows two variants and `DashboardPullRequest` grows three fields. Existing consumers compile against the wider type; the contract PR routes the new sort variants through the existing `Updated` ORDER BY so behaviour is preserved until Wave 2-D ships the new clauses.

### Negative

- **Per-account state, not per-device.** A viewer with two accounts sees different unread states for the same PR observed under each account. Correct given multi-account scoping, but jarring to users who think of "GitHub PR" rather than "(account, PR) pair".
- **Mention scan is naive.** Mentions inside fenced code blocks and inside blockquoted historical quotes count alongside real mentions. Documented and accepted; the watermark caps the false-positive cost to one bump per quoted instance.
- **In-memory search caps at the loaded row set.** A user typing a substring that matches a PR _not_ currently in the loaded view (e.g. archived PRs in a future archive surface) gets no result. v1 only loads the four primary views; this is acceptable until an archive view is added.
- **Composite recompute on every cycle.** Even for PRs with no relevant changes, the sync worker re-runs the `needs_attention` UPDATE. The cost is bounded (one UPDATE per (account, PR) per cycle) and well under the cycle's existing per-PR write overhead.
- **`pull_request_viewer_relations` row carries multiple concerns.** The row now mixes discovery-phase fields (`is_authored`, `is_review_requested`, `is_involved`, `last_seen_at`) with triage-phase fields (`read_at`, `mentioned_count_unread`, `needs_attention`). The naming keeps them disambiguated but a future cleanup ADR may split the table.

### Neutral / follow-ups

- **Explicit "Mark all read" action** is reserved for a polish PR after M4 ships. The auto-mark covers the common workflow; the explicit action is for power users with a backlog.
- **Mention chip on the row** stays optional in M4-F polish. The unread dot + `.pr.attention` row tint are the v1 attention signals; an explicit mention chip is duplicate visual weight unless user feedback flags ambiguity.
- **FTS5 search** is a fresh-ADR follow-up only if the bounded-dataset assumption breaks. The in-memory pipeline's interface (the store's `setSearch` + `filteredPullRequests` computed) doesn't need to change for a backend swap.
- **`@team-handle` resolution** waits on the team-membership ingestion work. The current `mentioned_count_unread` only tracks `@<viewer-login>` matches; team mentions would need a second column or a `mentioned_via` discriminator.
- **`needs_attention` recompute on lazy hydration.** The lazy hydrator already runs `mark_pr_read` semantics on drawer open; Wave 2-A also calls the recompute from inside `fetch_pr_conversation` so the dashboard reflects the new state before the next sync cycle.

## References

- Contract: [`docs/contracts/triage-ux.md`](../contracts/triage-ux.md)
- Migration: [`src-tauri/migrations/0010_triage_state.sql`](../../src-tauri/migrations/0010_triage_state.sql)
- ADR [0003](0003-local-storage-sqlite.md) - local SQLite storage that the triage columns extend.
- ADR [0004](0004-sync-polling-with-etag.md) - polling cadence; M4 adds zero new round-trips.
- ADR [0006](0006-graphql-first-rest-fallback.md) - GraphQL-first stance; M4 reuses already-persisted comment bodies.
- ADR [0010](0010-conversation-depth-storage.md) - thread storage that the mention scan reads from.
- ADR [0012](0012-threads-bar-four-state-and-outdated-counted.md) - threads-bar buckets feeding the "Unresolved threads" chip predicate.
- ADR [0013](0013-user-avatars-cache.md) - the `users` table layered on top of the same join surface.
