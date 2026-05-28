# 0030 — Per-check CI detail stored as a denormalised JSON column

- **Status:** Proposed
- **Date:** 2026-05-27
- **Issue:** [#426](https://github.com/cerinoligutom/PRism/issues/426)
- **Deciders:** @cerinoligutom

## Context

The dashboard CI badge (`CiBadge.vue`) renders a passing/total tally and a rollup state icon, projected from three denormalised columns on `pull_requests` (`ci_state`, `ci_total`, `ci_passing`; [ADR 0002 fields migration](../../src-tauri/migrations/0002_dashboard_fields.sql)). The per-check list arrives in the sync payload — `PR_DETAIL_QUERY` selects `statusCheckRollup.contexts(first: 100)` — but `enrichment/mod.rs::compute_ci_rollup` reduces it to the tally and discards the individual contexts. We currently request only `conclusion`/`status` on `CheckRun` and `state` on `StatusContext`; `name` and the details URL are never fetched.

Issue #426 asks to surface the individual checks, GitHub-style, so a viewer can see _which_ check failed without leaving PRism. This ADR decides how that per-check data is modelled and stored. It does not decide how the list is presented (see follow-ups).

## Decision drivers

- The per-check list is read as a whole, always for one PR at a time, and never filtered or joined by individual check. The access pattern is "give me all checks for this PR", not "find PRs where check X failed".
- The CI rate budget is unaffected: GitHub GraphQL charges per request, not per field, and we already fetch the `contexts` connection. Adding `name`/`detailsUrl` only grows payload size.
- The existing `ci_state`/`ci_total`/`ci_passing` rollup and the `ci_failing` triage chip (`triage/query.rs`) must keep working unchanged. The new data is additive.
- We want static analysis on both sides of the wire. The raw GraphQL shape is a tagged union (`CheckRun` | `StatusContext` | `Other`) with overlapping-but-different fields; consuming it directly in the UI would leak that awkwardness into TypeScript.
- Schema change should be a single additive, nullable column — no re-key, no new table to reconcile per cycle.

## Considered options

1. **Normalised `check_runs` table.** One row per check, FK to `pull_requests`, upsert + delete-orphans each sync, JOIN at dashboard query time. The "correct" relational model.
2. **Denormalised JSON column `ci_checks_json` on `pull_requests`.** Sync serialises the per-check list once; the dashboard projection deserialises it into the DTO.
3. **Re-fetch on demand.** Don't persist; issue a fresh GraphQL call when the user expands a PR's checks. No schema change, always-fresh, but adds a round-trip on interaction and a new failure mode, and the data is already in the cycle payload.

## Decision

**Option 2 — denormalised JSON column.**

- Migration `0025_ci_checks_json.sql` adds nullable `ci_checks_json TEXT` to `pull_requests`.
- `PR_DETAIL_QUERY` gains `name`/`detailsUrl` on the `CheckRun` inline fragment and `context`/`targetUrl` on `StatusContext`. The wire enum `StatusCheckContext` is extended to match and remains the GraphQL boundary type.
- Sync maps the wire enum into an **owned** type that both Rust and TypeScript define independently:

  ```
  CheckState = "success" | "failure" | "pending" | "neutral"
  CheckDetail = { name, state: CheckState, url: string | null }
  CiSummary gains: checks: CheckDetail[]
  ```

  Mapping from the wire shape: `SUCCESS -> success`; `FAILURE / ERROR / TIMED_OUT / STARTUP_FAILURE / ACTION_REQUIRED -> failure`; `NEUTRAL / SKIPPED / CANCELLED / STALE -> neutral`; anything in-flight or a `null` `CheckRun.conclusion` `-> pending`.
- `enrichment/mod.rs` serialises the `Vec<CheckDetail>` and writes it in the existing upsert. `dashboard/query.rs` appends `ci_checks_json` to both projection column lists (trailing, so existing row indices don't shift) and deserialises in `project_pr_row`, defaulting NULL or a parse error to an empty vec.

Why Option 2 over the others: the access pattern is whole-blob-per-PR, so a normalised table buys queryability we never use while costing a migration, per-cycle upsert/delete reconciliation, and a JOIN. Re-fetching on demand discards data we already pay to fetch and adds an interaction-time round-trip and failure mode. The JSON column matches how the data is used and keeps the change to one nullable column. The owned `CheckDetail` / `CheckState` types keep the GraphQL union's quirks at the boundary and give both the Rust DTO and the TypeScript mirror a flat, statically-checked shape.

## Consequences

### Positive

- One additive nullable column; no new table, no reconciliation, no JOIN. The rollup tally and `ci_failing` chip are untouched.
- The per-check data rides the existing detail cycle — no extra request, no interaction-time round-trip.
- The owned, mirrored `CheckState`/`CheckDetail` types give static analysis on both sides; the GraphQL tagged-union and GitHub's wide conclusion vocabulary collapse to four states at the sync boundary, in one place.

### Negative

- JSON in a column is opaque to SQL: we can't query or index by individual check. Accepted — that's not an access pattern we have or expect for v1.
- The four-state collapse is lossy. `neutral` merges skipped / cancelled / neutral; the UI won't distinguish them. Re-deriving a finer state later means a re-map in sync and a re-sync (the JSON is a cache, so a re-sync repopulates it).
- A future need to filter PRs by a specific check would force the normalised table after all — a migration away from this column.

### Neutral / follow-ups

- **Presentation is deliberately undecided.** The badge is a small target and a hover/click popover may not be the right fit. Candidates: click-to-open popover on the badge, inline row expansion, or a section in the conversation drawer (more room, consistent with where PR depth already lives). Decide before building the frontend; the backend is presentation-independent. This is a UI choice, not an architectural one, so it stays out of this ADR.
- The `passing`/`total` rollup stays the source of truth for the tally and the `ci_failing` chip; `checks` is supplementary detail, not a replacement.

## References

- [ADR 0006](0006-graphql-first-rest-fallback.md) — GraphQL-first protocol stance.
- [ADR 0016](0016-unified-multi-account-dashboard.md) — query-time rollup projection the CI fields ride along with.
- [ADR 0029](0029-sync-owns-conversation-persistence.md) — precedent for sync owning the canonical write of derived PR data in one transaction.
- Contract: [`docs/contracts/dashboard-data.md`](../contracts/dashboard-data.md)
- GitHub GraphQL: [`StatusCheckRollup`](https://docs.github.com/en/graphql/reference/objects#statuscheckrollup), [`CheckRun`](https://docs.github.com/en/graphql/reference/objects#checkrun), [`StatusContext`](https://docs.github.com/en/graphql/reference/objects#statuscontext)
