# 0004 — Sync strategy: polling with ETag / conditional requests

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#3](https://github.com/cerinoligutom/PRism/issues/3)
- **Deciders:** @cerinoligutom

## Context

PRism needs to keep the local cache fresh against GitHub's API. PRD §8.2 sets the rate-limit budget at under 20% of the 5000 req/hr per authenticated account; §8.3 sets the 95th-percentile freshness target at under 2 minutes stale at the original 60s default. The pre-v1 launch bumped the default to 5 minutes (see "Default interval" below) so the relaxed freshness expectation is now roughly the interval itself; users who need tighter freshness can dial down to 30s. §6 explicitly excludes a hosted backend in v1, which rules out webhook-driven push updates (they need a public callback URL).

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

We will use **polling with ETag / `If-Modified-Since` conditional requests**, run from a Rust worker on a configurable interval (default 5min, range 30s–10min). Per-resource ETag and last-modified values are stored in SQLite alongside the cached data. The worker is per-account isolated: one account's failure or rate-limit hit does not stall others.

The "Tracked" view (PRD §5.2, renamed from "Team" pre-v1) is per-repo opt-in to keep the budget under control for users in large orgs.

The interval itself persists on the `app_settings` singleton (column `sync_interval_seconds`) so the user's chosen cadence survives a relaunch. The worker reads this on startup before spawning per-account loops, falling back to the default if the column read fails. The `set_sync_interval` Tauri command writes the clamped value back after applying it in-memory.

Rationale: this is the only option that meets the v1 constraints. ETag 304 responses are cheap enough that the budget stays comfortably under 20% even at 30s intervals for typical accounts.

### Default interval

The original ADR shipped a 60s default. Pre-v1 we bumped this to 5 minutes (300s) because:

- Most users don't need sub-minute freshness for review dashboards. Mentions and "needs your attention" transitions surface via notifications (ADR 0017) so the dashboard reload cadence is for *background* refreshes, not critical alerts.
- 60s polling on a multi-account user with many tracked repos eats into the rate budget faster than the 20% guard alone is comfortable with.
- The slider in Settings → Sync still offers 30s through 10min, so power users who want fast refresh just pick a tighter value.

## Consequences

### Positive

- No hosted infrastructure.
- Predictable, transparent rate-limit consumption.
- Easy to debug: every fetch is a discrete HTTP call.

### Negative

- Up to 5 minutes latency vs. GitHub's native UI at default settings (or 30s if the user dials the interval all the way down). We surface this with a "last synced N ago" indicator (PRD §5.7) rather than pretending stale data is fresh.
- No real-time notifications without webhooks (deferred to post-v1).

### Neutral / follow-ups

- Conversation stats are computed incrementally per-thread (PRD §7.2) to keep sync passes cheap.
- Adaptive interval (back off on quiet repos, tighten on active ones) is a possible v1.1 enhancement.
- A post-v1 ADR would re-examine webhooks once a hosted relay is on the table.

## References

- [GitHub conditional requests](https://docs.github.com/en/rest/overview/resources-in-the-rest-api#conditional-requests)
- PRD §6, §7.2, §8.2, §8.3, §10
