# 0016 - Unified multi-account dashboard: dedupe-and-merge, query-time threads rollup, per-account failure isolation

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#163](https://github.com/cerinoligutom/PRism/issues/163)
- **Deciders:** @cerinoligutom

## Context

M5 was scoped as "Multi-account & GHE" on the assumption that multi-account support needed building. An audit before scoping showed it does not: the schema (per-(account, PR) relations, per-account `host`), the keychain backend (per-account entries), the sync worker (one task per account with hot add/remove via `AccountChangeListener`), PAT validation (`user_endpoint` already routes github.com vs GHE), the dashboard query (`account_id: Option<i64>` with `None` meaning union), and the frontend `accountFilter` ref are all already in place. The sidebar attention badge already fans out across every account.

What is missing is **semantics under the unified-view default** and **failure isolation when one account is sick**. ADR 0010 noted that `pull_requests.threads_(un)resolved_(un)involved` are per-account-of-last-sync and called out M5 as the revisit. The dashboard query documents that a PR authored by account A and review-requested for account B "shows up once under each" in the union case - the current per-relation row shape, not what a user with two identities on overlapping repos wants to see. There is no UI affordance to switch the `accountFilter`, even though the state and the per-account fan-out are wired.

Four sub-decisions need pinning before the implementation issues fan out:

1. **How should the threads rollup behave under multi-account?** The pre-aggregated columns (ADR 0010, refined to four buckets by ADR 0012) are written by whichever account synced the PR last, with that account's involvement test - which means the dashboard row flickers when account A and account B both have a relation.
2. **How should the unified view treat a PR multiple accounts have a relation to?** The current shape is one row per `(account, PR)` relation; a deduped-and-merged row is the alternative.
3. **What happens when one account is failing (401, rate-limited, network)?** The merged view must not corrupt or drop PRs from healthy accounts; mark-read writes must not roll back across accounts.
4. **What is GHE's M5 status?** The wiring exists, the validation cost does not fit the milestone.

## Decision drivers

- **Unified default.** "All accounts" is the first-launch surface and the most common ongoing scope. Single-account scoping is a filter, not the primary UX.
- **Per-account failure isolation is mandatory.** One account stuck on 401 must not freeze, blank, or duplicate the merged view from healthy accounts.
- **Read-state semantics under merged rows.** When a PR is visible from two accounts, "mark read" must apply per-account and tolerate partial failure.
- **No flickering rollups.** A signal that depends on "which account synced last" is a bug source. Either pre-aggregate per-account (schema change) or compute at query time.
- **Bounded query cost.** The dashboard reads a few hundred PRs at most; per-render aggregation over the threads tables is cheap if computed in one CTE rather than per-row.
- **Existing ADR contracts.** ADR 0010 (conversation-depth storage) and ADR 0012 (four-bucket threads bar) carry the per-account constraint forward. ADR 0015 (triage state) already keys read-state / mention-counts / `needs_attention` on `(account_id, pull_request_id)` - this ADR builds on that, doesn't replace it.
- **GHE validation cost.** End-to-end testing against a real Enterprise host, custom CA / SSO / proxy handling, and the documentation that promises support are out of M5's reach. The wiring stays.

## Considered options

### Threads rollup correctness

1. **Move the four-bucket rollup to `pull_request_viewer_relations`.** Per-account, written by the per-account sync cycle. Eliminates the cross-account overwrite. Costs a schema migration touching the hot row, four new columns per relation, and a recompute path on lazy hydration.
2. **Compute at query time from `review_threads` + `review_comments` against the in-scope account set.** Drops the cached columns. Uses one CTE per dashboard query. The involvement test becomes `c.author_login IN (SELECT login FROM accounts WHERE id IN (in-scope))`. Single code path for single-account, unified, and any future scope.
3. **Keep the current per-PR columns; accept the flicker.** Documented constraint. Wrong default under the new unified UX.

### Dashboard row shape in the union case

1. **Dedupe-and-merge.** `GROUP BY pr.id` when `account_id IS NULL`. Triage signals aggregate: `unread = MAX`, `needs_attention = MAX`, `mentioned_count_unread = SUM`, reviewers = union, account markers = up to N relation owners. One row per PR.
2. **One row per (account, PR) relation.** Current behaviour. A PR authored by Personal and review-requested on Work shows twice with different left-rail context.
3. **Hybrid: dedupe in Authored / Assigned, keep duplicates in Watching / Team.** Defensible (the author / reviewer relation is tighter than the watching net), but special-cases the SQL and the UX without a strong user-story payoff.

### Mark-read under merged rows

1. **Apply to every account with a relation, each write independent.** A partial failure persists the successful ones; the next cycle reconciles. Frontend optimistically flips the merged row once; the canonical state comes from the reload.
2. **Single transaction across all accounts.** All-or-nothing. A keychain hiccup on one account rolls back the others.
3. **Mark on the active account only.** Breaks the merged-row mental model - the user reads "the PR", not "the PR through account X". Leaves the row dotted on the other accounts.

### Failure isolation in the dashboard query

1. **`LEFT JOIN pull_request_viewer_relations` on the union path** (already used for Team view). A failing account whose relations got pruned doesn't drop the PR if another account also sees it.
2. **`INNER JOIN` plus client-side dedup.** Simpler SQL but the merged-row shape relies on the join not dropping rows.
3. **Per-account snapshots merged client-side.** Push the dedupe into TypeScript. Decouples SQL from merge logic but moves the four-bucket / SUM / MAX maths into the frontend with a worse test story.

### Account picker scope

1. **All accounts (unified) on first launch; persist last selection across restarts.** Matches the unified-default decision.
2. **First account on first launch; persist last selection.** Discoverable but biased toward single-account thinking.
3. **No persistence; reset to unified on every launch.** Surprising for users who routinely scope to one identity.

### GHE positioning

1. **Keep the wiring; descope validation; document "capable, unvalidated".** Costs nothing to leave the host routing in place; saves the test budget.
2. **Rip out host routing.** Regression; removes working code; closes a door for no real benefit.
3. **Ship as "GHE-supported".** Implies a validation level the test budget doesn't fund.

## Decision

### Threads rollup - option 2 (query-time computation)

Compute the four buckets in a CTE per dashboard query against `review_threads` + `review_comments`, scoped to the in-scope accounts:

```sql
WITH thread_buckets AS (
    SELECT t.pull_request_id,
           COUNT(*) AS total,
           SUM(CASE WHEN t.is_resolved = 0
                     AND EXISTS (SELECT 1 FROM review_comments c
                                  JOIN accounts a ON a.login = c.author_login
                                 WHERE c.review_thread_id = t.id
                                   AND a.id IN ({in_scope}))
                    THEN 1 ELSE 0 END) AS unresolved_involved,
           SUM(CASE WHEN t.is_resolved = 0
                     AND NOT EXISTS (...same subquery...)
                    THEN 1 ELSE 0 END) AS unresolved_uninvolved,
           SUM(CASE WHEN t.is_resolved = 1 AND EXISTS (...) THEN 1 ELSE 0 END) AS resolved_involved,
           SUM(CASE WHEN t.is_resolved = 1 AND NOT EXISTS (...) THEN 1 ELSE 0 END) AS resolved_uninvolved
      FROM review_threads t
     WHERE t.pull_request_id IN (loaded_pr_set)
     GROUP BY t.pull_request_id
)
SELECT pr.*, COALESCE(tb.total, 0), ..., FROM pull_requests pr
LEFT JOIN thread_buckets tb ON tb.pull_request_id = pr.id ...
```

`{in_scope}` is `(?)` for single-account or every tracked account id for unified. The CTE is bounded by `WHERE t.pull_request_id IN (loaded_pr_set)` so it never scans the full thread table.

The four `threads_*` columns on `pull_requests` become orphaned. SQLite cannot cheaply drop columns, so the migration sets them to `0` defaults and a code-side comment marks them legacy; a later cleanup ADR can do the column drop via the `12-step ALTER TABLE` recipe if the dead weight matters.

The `write_pr_updates` SQL that maintains the columns (worker.rs lines ~1273 onwards) gets removed - one fewer per-PR UPDATE in the cycle. Net cycle cost is lower, not higher, despite the query-time computation, because most PRs aren't being read every cycle.

### Dashboard row shape - option 1 (dedupe-and-merge)

In the union path (`account_id IS NULL`), the dashboard query GROUPs BY `pr.id` and aggregates:

| Signal | Merge rule |
|---|---|
| `unread` | `MAX` across relations (1 if any account is unread) |
| `needs_attention` | `MAX` across relations |
| `mentioned_count_unread` | `SUM` across relations |
| `reviewers` | union by login; states resolved as today's reviewer hydration does |
| `account_id` | dropped from the projection; replaced with `account_ids: int[]` |

The single-account path is unchanged - the `GROUP BY` only applies when no `account_id` filter is provided.

The frontend `DashboardPullRequest` DTO gains `account_ids: number[]` (length 1 in the filtered path, 1..N in the union path); `account_id: number` is removed. Row component reads `account_ids` to render up to two account avatar markers.

### Mark-read - option 1 (per-account, independent writes)

`mark_pr_read(pull_request_id)` resolves the relation set first, then issues one `UPDATE pull_request_viewer_relations` per `(account_id, pull_request_id)` in a single transaction. If the underlying write encounters an error (relation pruned mid-flight by a concurrent cycle), it logs and continues - the partial success persists. The frontend optimistically flips the merged row's `unread = false` and reloads after the call returns.

`mark_pr_unread(pull_request_id, account_id)` stays per-account because it's a power-user action invoked from an account-aware overflow menu; the merged-row affordance picks the originating account.

### Failure isolation - option 1 (LEFT JOIN union path)

The union path uses `LEFT JOIN pull_request_viewer_relations rel ON rel.pull_request_id = pr.id` so a failing account whose relations got pruned never drops PRs another account also sees. `pr.id` is the unique key; the merge aggregates over zero-or-more relation rows per PR.

The status bar already surfaces per-account `SyncPhase`. M5 adds a visual audit (sibling issue) confirming the failure state is distinguishable from "synced" at a glance - this ADR does not redesign the status bar.

### Account picker - option 1 (unified default, persisted)

Default scope on first launch is "All accounts". The last selected scope persists in the appearance store (alongside density and detail surface). The picker lives in the dashboard header, not the sidebar, so it sits with the chips + sort selector it composes with.

### GHE - option 1 (capable, unvalidated)

Keep the host column, keep `user_endpoint`, keep host-aware GraphQL / REST clients. Drop the M5 "GHE compatibility testing" line. The Architecture page, ADR 0005, and the M5 milestone description gain a note: PRism's code paths accept any GHE host that exposes `api/v3`, but PRism is not validated against a real GHE instance for v1.

## Consequences

### Positive

- **One code path for threads rollup.** Single-account and unified compute the buckets the same way. No per-account-of-last-sync flicker, no schema migration, no recompute path duplicated across worker + lazy hydrator.
- **Failure-isolated merged view.** A failing account contributes whatever rows it last surfaced; healthy accounts contribute their fresh ones. No cross-account corruption.
- **Cleaner row mental model.** One PR, one row, with account markers for context. Matches the way a user with Work + Personal thinks about their queue.
- **Cycle cost goes down, not up.** Dropping the per-cycle rollup UPDATE saves more than the per-query CTE costs at typical dashboard sizes.
- **GHE doesn't disappear.** Anyone who has a GHE host can still add it; the project just doesn't claim it's tested.

### Negative

- **Per-render computation for thread buckets.** Bounded by the loaded PR set (a few hundred typical, capped at the view's fetch). One CTE per query is not free, but it's microseconds on v1 sizes.
- **Orphaned columns on `pull_requests`.** The four `threads_*` columns become dead weight until a future cleanup ADR. SQLite's column-drop limitations make this an aesthetic concern, not a correctness one.
- **DTO breaking change.** `account_id: number` becomes `account_ids: number[]`. Affects every dashboard row consumer; mitigated by a single sweep in the M5 PR that introduces it.
- **Mark-read partial failures are silent.** A persistent per-account write failure shows up only when the next sync cycle reconciles; the user sees the row flip back to unread. Acceptable for v1; an explicit failure toast is a polish follow-up.
- **Same login on different hosts could match across accounts.** The mention scan and the reviewer-hydration's `accounts.login = comment.author_login` join can match across (login, host) pairs if a user has the same handle on github.com and a GHE host. M5 includes a sibling fix-ticket to tighten the join to `(login, host)`.

### Neutral / follow-ups

- **`threads_*` column drop.** A future cleanup ADR + migration removes the dead columns. Not urgent; tracked as a low-priority `chore`.
- **GHE re-promotion.** If someone validates PRism against a real GHE host (test fixtures, signed-off run-through, documentation), a follow-up ADR re-promotes the positioning. The wiring stays the same.
- **Per-account mark-read failure UX.** Surface partial failures via the existing reauth-banner pattern if the failure cause is a 401; silent retry on transient network errors.
- **Account picker accessibility.** Keyboard shortcut to switch scope (e.g. `cmd+1..9` for the Nth account, `cmd+0` for unified) is a polish follow-up.
- **FTS / archive views.** Out of M5. The in-memory search (ADR 0015) already caps at the loaded set; an archive surface would re-open that scoping decision.

## References

- ADR [0003](0003-local-storage-sqlite.md) - local SQLite storage.
- ADR [0004](0004-sync-polling-with-etag.md) - per-account polling isolation that this ADR builds on.
- ADR [0005](0005-pat-auth-and-keychain-storage.md) - PAT-in-keychain auth; this ADR adds a GHE-positioning note.
- ADR [0009](0009-pull-request-discovery-via-search-api.md) - per-account discovery feeding the relations table.
- ADR [0010](0010-conversation-depth-storage.md) - the pre-aggregated threads rollup that this ADR retires for the dashboard read path.
- ADR [0012](0012-threads-bar-four-state-and-outdated-counted.md) - the four-bucket shape preserved here, just computed at query time.
- ADR [0015](0015-triage-state-model.md) - the (account, PR) triage row that the merged-view aggregations read.
- Contract: [`docs/contracts/dashboard-data.md`](../contracts/dashboard-data.md) - dashboard DTO + query contract; M5 PRs update it in lockstep.
