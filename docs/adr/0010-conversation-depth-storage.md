# 0010 — Conversation-depth storage and hydration

- **Status:** Accepted
- **Date:** 2026-05-20
- **Issue:** [#68](https://github.com/cerinoligutom/PRism/issues/68)
- **Deciders:** @cerinoligutom

## Context

M3 ships the conversation surface: per-thread state, conversation stats, comment-type breakdown, and per-thread previews. M1 created `review_threads`, `review_comments`, `issue_comments`, and `reviews` tables (issue #10), but the M2 sync worker writes none of them — the conversation tables have been empty in production since M1. M3 is the first cycle where this data actually lands in SQLite, so the storage shape is open.

Three storage decisions need pinning before the M3 wave-2 implementers branch out:

1. **How threads are identified.** The local `INTEGER PRIMARY KEY` rowid is fine for foreign keys but useless for upserts when the same thread reappears in a later sync cycle. GitHub's GraphQL `ReviewThread` exposes only a global node ID (a string); there's no `databaseId`. The same gap exists in `reviews`. `review_comments`, `issue_comments`, and `reviews` do expose `databaseId` but the GraphQL node ID is uniformly available across all four. The upsert path needs a consistent key.

2. **How the dashboard row reads thread rollup counts.** The row threads bar shows `unresolved / total` per PR. Two shapes work: (a) compute the rollup at query time via a sub-aggregation per row, or (b) pre-aggregate into columns on `pull_requests` written by the sync worker. The dashboard list query already pre-aggregates CI counts (`ci_total`, `ci_passing`); the question is whether to extend the pattern or diverge.

3. **How comment bodies are pulled.** PRs can carry hundreds of comments. Pulling every comment body in every sync cycle multiplies the steady-state `PR_DETAIL_QUERY` response size. The opposite extreme — fetch nothing in the cycle, fetch everything on detail open — leaves the row's threads bar without a head-comment snapshot. The scoping discussion landed on a middle path; this ADR records it.

## Decision drivers

- Per-account sync cycle budget remains under 20% of 5000 req/hr (PRD §8.2, ADR 0004).
- Dashboard row renders without a per-row sub-aggregation; the list query stays the shape M2 established.
- Drawer / route open completes in a single GitHub round-trip when possible.
- Upserts survive thread / review removal on GitHub (deleted threads disappear from the list).
- Multi-account doesn't double-write thread rows (threads belong to PRs, not accounts).
- The schema migration is forward-only and minimally invasive to existing rows.

## Considered options

### Thread / review identification

1. **Keep `INTEGER PRIMARY KEY` only, populate with the GraphQL `databaseId`.** Fails for `ReviewThread` — no `databaseId` exists.
2. **Replace `INTEGER PRIMARY KEY` with `TEXT PRIMARY KEY` using node ID.** Breaks existing foreign keys (`review_comments.review_thread_id` is `INTEGER`); requires migrating every dependent column.
3. **Keep `INTEGER PRIMARY KEY` (rowid) + add `node_id TEXT UNIQUE` for upserts.** No FK breakage; one new column per affected table; upserts target `node_id`.
4. **Hash the node ID into an INTEGER.** Adds a layer of indirection, makes debug joins harder, and gains nothing over option 3.

### Dashboard row rollup

1. **Compute rollups at query time.** Three sub-aggregations per row (total / unresolved / involved). The `involved` aggregation joins `accounts` and `review_comments`; per-row cost scales with comment count.
2. **Pre-aggregate into columns on `pull_requests` written by the worker.** Matches M2's `ci_total` / `ci_passing` pattern. One UPDATE per PR per cycle; row reads stay cheap.
3. **Materialised view.** SQLite supports neither materialised views nor incremental aggregation. Triggers could simulate it; the complexity isn't justified.

### Comment-body hydration

1. **Full pull every cycle.** Every comment body in every thread + every issue comment + every review body, every cycle. Cycle size grows roughly linearly with comment density. Detail open is instant.
2. **Stale-aware refresh.** Pull full bodies only when `pull_requests.updated_at` advanced since the last cycle. Most steady-state cycles pay zero; touched PRs pay the full cost.
3. **Capped + lazy.** Cycle pulls thread headers + head-comment snapshot + counts. Full bodies fetched by a `fetch_pr_conversation` command on drawer / route open. Cycle size near-constant; detail open pays a single round-trip.

## Decision

**Identification.** Add `node_id TEXT UNIQUE` to `review_threads`, `review_comments`, `issue_comments`, and `reviews`. Keep the existing `INTEGER PRIMARY KEY` (rowid) for foreign keys. Upserts target `node_id`; pruning deletes rows whose `node_id` doesn't appear in the latest fetch.

`review_comments` and `issue_comments` also get `database_id INTEGER`, populated from `databaseId` when GraphQL surfaces it, for future cross-referencing against REST endpoints — not required for M3 reads, but cheap to persist and removes a migration step from any future REST fallback path.

**Dashboard row rollup.** Pre-aggregate. Add `threads_total`, `threads_unresolved`, `threads_involved` columns to `pull_requests`, written by the sync worker inside `write_pr_updates` after the thread upserts have committed. The dashboard list query reads them as scalar columns alongside `ci_total` / `ci_passing` — same pattern, same cost shape.

**Comment-body hydration.** Capped + lazy.

- The sync cycle's `PR_DETAIL_QUERY` pulls `reviewThreads(first:100)` with `comments(first:1)` head + `totalCount`, `reviews(first:30)` with bodies, and `issueComments(first:50).totalCount`. Head-comment snapshot columns on `review_threads` carry the row preview.
- A new Tauri command `fetch_pr_conversation(pull_request_id)` issues `PR_COMMENTS_QUERY` (a separate GraphQL string defined by M3-B) to pull full comment bodies + issue-comment bodies. Called by the drawer / route on mount; results persisted into `review_comments` and `issue_comments`.
- The lazy fetch is idempotent: the conversation store de-duplicates concurrent mounts, and the SQL inserts use `INSERT ... ON CONFLICT(node_id) DO UPDATE` so re-fetches don't double-write.

Review bodies (the "summary" comment-type) ship in the cycle because they're cheap (one body per submitted review, capped at 30) and the comment-type breakdown tile reads them at stats time. Pulling them lazily would leave the tile blank until the drawer opens, which is the same downside as for review-comment counts but without the size justification.

## Consequences

### Positive

- The dashboard list query gains one scalar column per rollup; performance shape unchanged.
- Steady-state cycle size stays close to M2's; the new GraphQL fields add a fixed-bounded ~20–40% to per-PR response payload, not a comment-count multiplier.
- Drawer / route open is one GitHub round-trip in the worst case (capped at 200 comments + 200 issue comments per PR), instant in the cache-hit case.
- Upserts handle thread / review removal cleanly via `node_id`-based pruning.
- Multi-account safe: thread / comment / review rows are PR-scoped, written once; only `threads_involved` is per-account, computed at write time against the cycle's active account.

### Negative

- Comment-type breakdown's "review" count (count of `review_comments`) reads as zero on PRs whose drawer / route has never been opened, until the first lazy hydration. The other tiles (oldest unresolved, avg response, resolution rate, summary count, issue count) are correct from cycle 1 because they read either thread-level columns or the `issue_comments_count` rollup. Documented in the contract; acceptable for v1.
- `threads_involved` is per-account; PRs touched by multiple accounts overwrite the count on each cycle. Multi-account users see the count for the most recently synced account, not the union. M5 (multi-account) revisits.
- The `node_id` columns are nullable to preserve the existing migration history (`0001_init.sql` rows seeded by tests have NULL node IDs). Partial unique indexes (`WHERE node_id IS NOT NULL`) enforce uniqueness for populated rows.

### Neutral / follow-ups

- If lazy-fetch latency becomes a complaint, the stale-aware option remains available without a schema change — just a strategy switch in the worker.
- Inline expansion (the deferred third detail-surface option) reads from the same lazily-hydrated data; no storage change required when it lands.
- The conversation-stats math could be pre-aggregated onto `pull_requests` (oldest_unresolved_at, etc.) if the read-time computation becomes a hot spot. Not justified pre-launch; revisit if M7 hardening surfaces it.

## References

- [GitHub GraphQL: PullRequest.reviewThreads](https://docs.github.com/en/graphql/reference/objects#pullrequest)
- [GitHub GraphQL: PullRequestReviewThread](https://docs.github.com/en/graphql/reference/objects#pullrequestreviewthread)
- ADR [0004](0004-sync-polling-with-etag.md) — sync polling cadence and rate budget.
- ADR [0006](0006-graphql-first-rest-fallback.md) — GraphQL-first protocol stance.
- ADR [0009](0009-pull-request-discovery-via-search-api.md) — the contract-PR pattern M3 mirrors.
- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md)
