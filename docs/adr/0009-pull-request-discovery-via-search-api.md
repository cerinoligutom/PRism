# 0009 — Pull-request discovery via GitHub Search API

- **Status:** Accepted
- **Date:** 2026-05-20
- **Issue:** [#35](https://github.com/cerinoligutom/PRism/issues/35)
- **Deciders:** @cerinoligutom

## Context

M2 ships the four sidebar views (Authored / Assigned / Watching / Team). The sync worker built in M1 enriches PRs already in the `pull_requests` table, but there's no discovery step — no code path that asks GitHub which PRs the viewer authored, has been review-requested on, or has touched. Without discovery every view renders empty.

The four views differ in their relationship to the viewer:

- **Authored** — PRs where `author = viewer`.
- **Assigned** — PRs where the viewer is a requested reviewer and has not submitted a review.
- **Watching** — PRs where the viewer is involved in any way (author, assignee, reviewer, commenter, mentionee, reactor, subscriber); per PRD §5.5 / wiki Architecture §Auto-tracking.
- **Team** — open PRs in a repo the user has explicitly opted in to track.

## Decision drivers

- API budget envelope is < 20% of 5000 req/hr per account (PRD §8.2; ADR 0004).
- Each query must list every PR matching its relation, across every repo the viewer can access.
- New relations (a fresh review request, a new mention) must surface within one sync cycle.
- A single PR can be authored by account A and review-requested for account B — discovery must keep these relations distinct per account.

## Considered options

1. **REST `/issues?filter=created|assigned|mentioned|subscribed`** — single endpoint per filter; lacks a `review-requested` filter and conflates issues with PRs.
2. **REST `/repos/{owner}/{name}/pulls` per known repo** — works only when every relevant repo is enumerated in advance; cost scales linearly with repo count.
3. **GraphQL `viewer.pullRequests`** — covers Authored cleanly; no built-in filter for `review-requested` or `involves`.
4. **GraphQL Search (`is:pr is:open <qualifier>:@me`)** — one query per view, server-side filter, cursor pagination. Covers the three user-centric views.

## Decision

Use **GraphQL Search** for the three user-centric views (Authored / Assigned / Watching) and **REST `/repos/{owner}/{name}/pulls`** for the per-repo Team view. Discovery runs as the first phase of each sync cycle and feeds the existing per-PR enrichment loop.

Per cycle, per account, the rate-budget envelope is:

- 3 GraphQL Search queries (one per relation flag) — ~3 requests baseline.
- Pagination adds 1 request per 50 results past the first page; capped at 200 results per query (4 pages) in v1.
- N REST `/pulls` calls for N team-tracked repos (typically 1 per repo).
- Detail fetches scale with discovered PRs, hard-capped at 100 PRs per cycle.

For a viewer with 30 active PRs across 10 team-tracked repos: ~3 + ~30 (detail) + ~10 (team) = ~43 requests per cycle, or ~14% of the per-hour budget at the default 5-minute interval.

PR-viewer relations are stored in a new `pull_request_viewer_relations` table keyed by `(account_id, pull_request_id)`, with `is_authored`, `is_review_requested`, `is_involved` flags and a `last_seen_at` timestamp. Each cycle rebuilds the relation flags from search results; rows whose `last_seen_at` predates the current cycle are pruned (the viewer no longer has that relationship).

The Team view does not write to this table — it joins on `repos.is_team_tracked = 1`, since the team relationship is a property of the repo, not the (account, PR) pair.

## Consequences

### Positive

- All four views populate from a single sync cycle. No per-repo enumeration for user-centric views.
- Fresh relations (new review requests, mentions) appear within one cycle.
- Multi-account is clean — each account's relations are scoped by `account_id`.
- The 20% rate-budget target holds for typical viewer workloads.

### Negative

- GraphQL Search has its own rate limit (30 req/min per token) separate from the core 5000/hr limit. High-frequency manual refreshes from the UI could trip it; the existing rate-budget guard handles this by short-circuiting cycles when remaining budget drops below the configured threshold.
- Search results don't list relations the viewer just lost (e.g. un-assigned). Pruning via `last_seen_at` handles this on the next cycle, leaving a one-cycle window where stale relations persist.
- Three extra queries per cycle per account; the arithmetic above accepts that cost.

### Neutral / follow-ups

- A future ADR may revisit the rate-budget envelope if Search-API limits tighten or per-account PR counts grow beyond the 100-per-cycle cap.
- M5 (multi-account + GHE) needs to confirm GHE 3.x exposes `search.type: ISSUE` with the same qualifier set.

## References

- [GitHub GraphQL API: search](https://docs.github.com/en/graphql/reference/queries#search)
- [GitHub search qualifiers for PRs](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests)
- ADR [0004](0004-sync-polling-with-etag.md) — sync polling cadence and rate budget.
- ADR [0006](0006-graphql-first-rest-fallback.md) — GraphQL-first protocol stance.
- Contract: [`docs/contracts/dashboard-data.md`](../contracts/dashboard-data.md)
