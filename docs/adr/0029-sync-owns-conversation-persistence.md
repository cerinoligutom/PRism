# 0029 — Sync owns conversation persistence

- **Status:** Accepted
- **Date:** 2026-05-26
- **Issue:** [#422](https://github.com/cerinoligutom/PRism/issues/422)
- **Deciders:** @cerinoligutom

## Context

[ADR 0010](0010-conversation-depth-storage.md) split conversation persistence into two write paths: the sync cycle's `PR_DETAIL_QUERY` writes thread headers + a head-comment snapshot column on `review_threads`, and a Tauri command `fetch_pr_conversation` triggered by the drawer / route open writes the full `review_comments` and `issue_comments` rows. The split kept the steady-state cycle cheap.

Issue #422 surfaced the cost: every downstream signal that reads `review_comments` or `issue_comments` is silently stale until the drawer has been opened on that PR. The audit lists four broken user-visible signals:

1. Dashboard threads bar — involvement bucket (orange / green) reads as uninvolved (red / blue) until hydration.
2. Conversation header threads bar — same query.
3. `@you` mention notifications and the unread mention counter — the watermark scan in `triage_recompute/mod.rs` walks two empty tables.
4. `needs_attention` composite, signal #1 ("authored PR has unresolved involved thread") — same empty-table cause.

(1) is visually loud and reproducible. (3) silently drops desktop notifications. Both are correctness gaps, not UX polish.

The split was made for bandwidth reasons (PRD §8.2, ADR 0004). Re-examining: GitHub GraphQL charges per request, not per field, so adding comment fields to `PR_DETAIL_QUERY` does not move the rate budget. The actual cost is payload size, which is cheap by comparison.

## Decision drivers

- Correctness of derived signals must not depend on the user having opened the drawer first.
- One canonical write path per table (SRP). Two writers fighting over the same rows is the structural cause of #422.
- Sync remains under 20% of the per-account rate budget (ADR 0004, unchanged).
- Drawer open stays fast; a synchronous DB read is faster than a GraphQL round-trip.
- Schema changes should be additive or simple drops, not a re-key.

## Considered options

1. **Plug the head-comment gap only.** Sync also writes the head comment to `review_comments`. Cheap. Leaves involvement detection wrong whenever the viewer's only comment is a reply (not the head) and the drawer hasn't been opened. Mention scan still misses replies and every issue comment.
2. **Widen the involvement predicate to read `review_threads.head_comment_author_login`.** Cheaper diff. Involvement definition now lives in two places (`review_comments` and a denorm column). Mention scan and `needs_attention` signal #1 still broken.
3. **Sync owns conversation persistence.** Sync's `PR_DETAIL_QUERY` fetches the full comment fields with bodyHTML and diffHunk; sync persists `review_comments` and `issue_comments` in the same transaction as `review_threads`. The lazy hydrator is deleted. Drawer reads from cache.

## Decision

**Option 3.** Sync becomes the canonical writer for all conversation tables.

- **`PR_DETAIL_QUERY` expansion.** `reviewThreads.comments(first: 100)` selects the full field set previously held by `PR_COMMENTS_QUERY` (`id, url, databaseId, author { login avatarUrl }, body, bodyHTML, bodyText, createdAt, path, line, originalLine, diffHunk`). `issueComments(first: 100)` selects the full field set (`id, url, databaseId, author { login avatarUrl }, body, bodyHTML, createdAt`). Both connections expose `pageInfo` so the worker can paginate.
- **Outer pagination in sync, capped at 4 pages each.** A defensive backstop: 400 threads / 400 issue comments per PR is well above the hydrator's prior reach and any realistic v1 PR. PRs exceeding the cap truncate at the boundary; the same shape the hydrator carried at `MAX_PAGES = 8`.
- **Inner comments stay at `first: 100`, no inner pagination.** Matches the hydrator's existing coverage; threads with more than 100 comments truncate at 100. The contract carried this same cap before; the change is who fetches them, not how many.
- **`write_pr_updates` writes `review_comments` and `issue_comments`** inside the existing transaction. The hydrator's upserts (`upsert_review_comment`, `upsert_issue_comment`, `update_thread_diff_hunk`, `upsert_user_avatar`) move to a shared module.
- **`fetch_pr_conversation` / `PR_COMMENTS_QUERY` are deleted.** The conversation drawer reads `HydratedConversation` from a synchronous DB query. The frontend conversation store keeps its in-memory cache as a render shortcut; the `handleSyncedCycle` re-hydration scaffolding goes away.
- **`review_threads.head_comment_*` denorm columns are dropped** in a migration. The list query joins `review_comments ORDER BY created_at ASC LIMIT 1` for the head row.
- **Refresh signal collapses to `dashboard://refresh`.** Sync emits it at the end of every successful cycle. The dashboard store's `phase === "synced"` listener and the conversation store's sync-cycle handling fold into the single refresh listener.

This ADR supersedes the "Capped + lazy" decision in [ADR 0010](0010-conversation-depth-storage.md) under "Comment-body hydration"; the identification (`node_id`-keyed upserts) and the dashboard rollup (now query-time per ADR 0016) carry forward unchanged.

## Consequences

### Positive

- The four broken signals in #422 fix as a single change: the dashboard threads bar, the conversation header threads bar, mention notifications, and `needs_attention` signal #1 all derive from tables sync now keeps current.
- One write path per table. The dual-writer ambiguity that caused #422 disappears.
- Drawer open is a DB read, not a GraphQL round-trip. The drawer-load failure mode is gone.
- `PR_DETAIL_QUERY` payload grows by comment bodies (text + HTML) per cycle, but request count is unchanged so the rate budget is unchanged.
- One refresh signal across surfaces; the two-listener pattern in the dashboard store collapses.

### Negative

- The detail body-hash cache (#234) re-hashes on first cycle after the schema change. Every PR refetches once. One-shot.
- Comment-body payload over the wire goes up. For a typical PR (5 threads, ~3 comments each, ~10 issue comments) this is a few KB of bodyHTML + diffHunk added to the existing detail response. For pathological PRs with hundreds of long comments, payloads can hit a few hundred KB.
- The migration to drop `review_threads.head_comment_*` uses inline `ALTER TABLE DROP COLUMN` (SQLite 3.35+, well below the version rusqlite 0.39's `bundled` feature ships).

### Neutral / follow-ups

- If steady-state payload size becomes a concern, the stale-aware refresh mentioned in ADR 0010 remains available as a follow-up — skip the comment fields when `pull_requests.updated_at` hasn't moved. Not justified pre-launch.
- Real-time mid-cycle refresh is still out of scope; the dashboard reloads at end-of-cycle. The user value is correctness of what gets shown when the reload lands, not per-PR liveness during the cycle.

## References

- [ADR 0004](0004-sync-polling-with-etag.md) — sync polling cadence and rate budget.
- [ADR 0006](0006-graphql-first-rest-fallback.md) — GraphQL-first protocol stance.
- [ADR 0010](0010-conversation-depth-storage.md) — superseded in part by this ADR.
- [ADR 0012](0012-threads-bar-four-state-and-outdated-counted.md) — four-bucket rollup.
- [ADR 0015](0015-triage-state-model.md) — mention detection and `needs_attention` composite.
- [ADR 0016](0016-unified-multi-account-dashboard.md) — query-time threads rollup.
- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md)
