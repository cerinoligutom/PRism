# 0007 — Status timeline derived from the timeline events API

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#6](https://github.com/cerinoligutom/PRism/issues/6)
- **Deciders:** @cerinoligutom

## Context

PRism surfaces "time since latest status change" per PR (PRD §5.3), where a status change is one of:

- draft ↔ ready for review
- review decision flips (approved ↔ changes_requested)
- merge-state changes (mergeable ↔ conflicts ↔ merged/closed)

GitHub does **not** expose a single "status changed at" timestamp. The information is reconstructible from the [timeline events API](https://docs.github.com/en/rest/issues/timeline), which lists every event on an issue or PR. The relevant events are `ready_for_review`, `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`, and `reopened`. The most recent qualifying event becomes the PR's "latest status change" timestamp.

The timeline events API is REST-only (ADR 0006). Each event is paginated; for PRs with thousands of events (long-lived branches in large orgs), we need pagination handling.

## Decision drivers

- PRD §5.3 requires this surface; it is a primary user-visible value.
- No single GitHub field provides it.
- The derivation logic must be testable and deterministic.
- Sync cost must stay inside the rate-limit budget (ADR 0004) — pagination is the main risk.

## Considered options

1. **Skip the surface** — eliminated; it's a primary PRD requirement.
2. **Approximate from `updated_at`** — incorrect; `updated_at` flips on any event including comments.
3. **Derive from timeline events** — correct but adds sync complexity.

## Decision

We will **derive "latest status change" from the timeline events API**:

- Qualifying event types: `ready_for_review`, `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`, `reopened`.
- Algorithm: walk the events newest-first; return the timestamp of the first qualifying event encountered.
- Pagination: stop early as soon as a qualifying event is found, or when older events would be useless.
- Persistence: the derived timestamp is stored on the PR row in SQLite (ADR 0003) and recomputed when new timeline events arrive for that PR.

## Consequences

### Positive

- Surface is correct and matches what users care about, not a proxy.
- Derivation is localised to one function with table-driven tests.

### Negative

- Extra REST calls per PR (mitigated by ETag conditional requests).
- Algorithm needs updating if GitHub adds new event types relevant to status.

### Neutral / follow-ups

- The set of qualifying events is a finite enum; surface it as a Rust type so the test suite catches drift.
- A future ADR may add adaptive pagination heuristics if rate-limit consumption becomes an issue for very long-lived PRs.

## References

- [GitHub timeline events API](https://docs.github.com/en/rest/issues/timeline)
- ADR 0004 (sync polling), ADR 0006 (GraphQL-first / REST fallback)
- PRD §5.3, §7.3
