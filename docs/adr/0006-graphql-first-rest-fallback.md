# 0006 — GitHub API: GraphQL-first with REST fallback

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#5](https://github.com/cerinoligutom/PRism/issues/5)
- **Deciders:** @cerinoligutom

## Context

PRism's PR detail rows show conversation depth, including resolved vs. unresolved review threads (PRD §5.3). The resolved state of a review thread is exposed by GitHub **only via GraphQL** (`pullRequestReviewThreads.isResolved`). REST does not expose it. Other PR data (lists, basic detail, check runs) is available in both, but GraphQL allows fetching the right shape in one round trip, which matters for rate-limit budget (ADR 0004).

The timeline events API used for status reconstruction (PRD §7.3) is REST-only.

## Decision drivers

- Resolved-thread state is GraphQL-only.
- Round-trip efficiency under rate-limit budget (ADR 0004).
- Timeline events API is REST-only (ADR 0007).
- Library and tooling coverage in Rust for both protocols.

## Considered options

1. **REST-only** — eliminated by missing resolved-thread state.
2. **GraphQL-only** — eliminated by GraphQL's lack of coverage for timeline events.
3. **GraphQL-first, REST fallback** — use GraphQL where it covers; REST only for endpoints GraphQL doesn't expose.

## Decision

We will use **GraphQL as the primary protocol** for PR detail, reviews, threads, and comments, and **REST only for endpoints GraphQL does not cover** — currently the timeline events API and any auxiliary REST-only endpoint we discover. ETag / `If-Modified-Since` conditional requests apply to REST per ADR 0004; GraphQL queries are batched and shaped to minimise round trips.

A shared rate-limit accounting layer tracks consumption across both clients per account.

## Consequences

### Positive

- Conversation state is complete (resolved threads available).
- Fewer round trips per PR fetch.
- One auth layer, two protocols.

### Negative

- Two clients to maintain.
- GraphQL schema changes require query updates; we accept that GitHub's GraphQL API is stable enough to live with this.

### Neutral / follow-ups

- Per-query ETag-equivalent caching for GraphQL is partial (GitHub returns a per-query node id; we cache responses keyed by query+variables hash for short windows).
- A future ADR may consolidate around a single Rust GitHub client crate once the choice is clearer.

## References

- [GitHub GraphQL API: pullRequestReviewThreads](https://docs.github.com/en/graphql/reference/objects#pullrequestreviewthread)
- [GitHub timeline events REST API](https://docs.github.com/en/rest/issues/timeline)
- PRD §5.3, §7.2, §10
