# 0004 — Sync strategy: polling with ETag / conditional requests

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#3](https://github.com/cerinoligutom/PRism/issues/3)
- **Deciders:** @cerinoligutom

## Context

PRism needs to keep the local cache fresh against GitHub's API. PRD §8.2 sets the rate-limit budget at under 20% of the 5000 req/hr per authenticated account; §8.3 sets the 95th-percentile freshness target at under 2 minutes stale at the default 60s sync interval; §6 explicitly excludes a hosted backend in v1, which rules out webhook-driven push updates (they need a public callback URL).

The cache lives in SQLite (ADR 0003) and the storage layer can hold per-resource ETags / `Last-Modified` values.

## Decision drivers

- No hosted backend in v1 (PRD §6).
- Rate-limit budget under 20% of 5000 req/hr per account (PRD §8.2).
- 95th-percentile freshness under 2 minutes (PRD §8.3).
- Failed syncs must be surfaced, not hidden (PRD §8.3, §10).
- Scaling concern: users in large orgs with 1000+ repos must remain inside the budget (PRD §10).

## Considered options

1. **Webhook push updates** — instant freshness, requires a public hosted callback. Out of scope.
2. **Polling without conditional requests** — burns rate-limit budget on unchanged resources.
3. **Polling with ETag / `If-Modified-Since`** — 304 responses cost ~1% of a normal response budget; standard for GitHub.
4. **GraphQL subscriptions** — GitHub does not offer them.

## Decision

We will use **polling with ETag / `If-Modified-Since` conditional requests**, run from a Rust worker on a configurable interval (default 60s, range 30s–10min). Per-resource ETag and last-modified values are stored in SQLite alongside the cached data. The worker is per-account isolated: one account's failure or rate-limit hit does not stall others.

The "Team / org-wide" view (PRD §5.2) is per-repo opt-in to keep the budget under control for users in large orgs.

Rationale: this is the only option that meets the v1 constraints. ETag 304 responses are cheap enough that the budget stays comfortably under 20% even at 30s intervals for typical accounts.

## Consequences

### Positive

- No hosted infrastructure.
- Predictable, transparent rate-limit consumption.
- Easy to debug: every fetch is a discrete HTTP call.

### Negative

- Up to 60s latency vs. GitHub's native UI at default settings. We surface this with a "last synced N ago" indicator (PRD §5.7) rather than pretending stale data is fresh.
- No real-time notifications without webhooks (deferred to post-v1).

### Neutral / follow-ups

- Conversation stats are computed incrementally per-thread (PRD §7.2) to keep sync passes cheap.
- Adaptive interval (back off on quiet repos, tighten on active ones) is a possible v1.1 enhancement.
- A post-v1 ADR would re-examine webhooks once a hosted relay is on the table.

## References

- [GitHub conditional requests](https://docs.github.com/en/rest/overview/resources-in-the-rest-api#conditional-requests)
- PRD §6, §7.2, §8.2, §8.3, §10
