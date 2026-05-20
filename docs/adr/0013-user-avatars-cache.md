# 0013 — User avatar caching via a `users` table

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#99](https://github.com/cerinoligutom/PRism/issues/99)
- **Deciders:** @cerinoligutom

## Context

Every avatar in the dashboard and conversation surface renders as initials inside a slot-coloured circle. The fallback is deterministic and recognisable, but at a glance it doesn't tell you who's commenting, requesting a review, or merging — three of the most common signals reviewers want from the row. GitHub already serves avatar images via its public CDN; we need a way to cache the URL locally so every render of a login resolves to the same image without a per-render HTTP probe.

Three storage decisions need pinning:

1. **Where to store the avatar URL.** Per-author-table columns (`pull_requests.author_avatar_url`, `reviews.reviewer_avatar_url`, `requested_reviewers.avatar_url`, `review_comments.author_avatar_url`, ...) duplicate the same data across every table that mentions a login. A dedicated `users` table keeps the URL in one place and joins it back via the login.
2. **When to populate.** The sync cycle's `PR_DETAIL_QUERY` already touches every login that matters for a PR (author + thread/issue comment heads + review authors + requested reviewers + timeline actors). Extending those GraphQL selections to include `avatarUrl` and writing the resulting `(login, avatar_url)` pairs on the same cycle adds zero new round-trips.
3. **How to read.** Read-side queries already project per-PR / per-thread DTOs; adding a `LEFT JOIN users` per author column resolves the avatar URL at query time without restructuring the DTO shape.

Image caching itself is handled by the system webview — once an `<img src="https://avatars.githubusercontent.com/...">` lands on the page, GitHub's CDN response headers carry the browser-level cache controls. We don't store image bytes locally; we store URLs (~80–100 bytes each).

## Decision drivers

- One row per login regardless of how many comments / reviews reference that login — duplicating `avatar_url` across `pull_requests`, `reviews`, `review_comments`, `issue_comments`, `requested_reviewers`, `timeline_events` is six places to keep in sync.
- Avatar URLs change rarely (account avatar updates) but the row can survive a stale URL — the `<img>` `onerror` falls back to initials and the next sync rewrites it.
- Multi-account scaling: the same login can appear under two accounts; the cached avatar URL must be account-agnostic.
- GraphQL `avatarUrl` is a public asset; storing it locally adds no new PII beyond what `github.com/<login>` already exposes.
- The cycle's rate budget envelope must not move: adding `avatarUrl` to existing selections is a payload-size delta, not a request-count delta.

## Considered options

### Storage shape

1. **`avatar_url` column on every author-bearing table.** Each table that carries a login (`pull_requests.author_login`, `reviews.reviewer_login`, `review_comments.author_login`, `issue_comments.author_login`, `requested_reviewers.login`, `timeline_events.actor_login`, `head_comment_author_login` on `review_threads`) grows a sibling column. Six write sites need to repeat the same URL when a comment + review + timeline event reference the same login.
2. **Dedicated `users(login PRIMARY KEY, avatar_url, last_seen_at)` table.** Single source of truth. Read queries `LEFT JOIN users ON users.login = ...author_login`. One write site per cycle per unique login.
3. **Cache only the most recently seen avatar URL on the PR row.** Smallest schema delta but breaks down once a comment-thread author isn't the PR author.
4. **No persistence; resolve `avatarUrl` per render via Avatar API.** Adds a network probe per row render; the CDN response is fast but the request count balloons on the dashboard.

### Population path

1. **Extend the existing `PR_DETAIL_QUERY` + REST timeline + discovery query to select `avatarUrl` on every `author` / `actor` / `requestedReviewer { ... on User }` branch.** Same cycle, same round-trip count.
2. **Add a separate `users` GraphQL query.** One extra round-trip per cycle to enumerate logins; redundant with the data already returned by the per-PR queries.
3. **Backfill from the REST users endpoint on first encounter.** A second round-trip per never-seen login on each cycle; doesn't survive a token rotation or 401.

### Read-side resolution

1. **`LEFT JOIN users` at query time per author column.** Existing queries grow one join per surfaced login; missing rows surface `avatar_url = None` cleanly.
2. **Materialise the URL into the row at write time.** Duplicates option (storage 1) — multiple write sites, mismatched if the cycle's payload is partial.

## Decision

**Storage.** Add a `users(login TEXT PRIMARY KEY, avatar_url TEXT, last_seen_at INTEGER NOT NULL DEFAULT 0)` table. One row per login, account-agnostic. Sync writes UPSERT on `login`; only entries with a populated `avatar_url` are written so a partial payload can't blank a previously-populated row with a NULL.

**Population.** Extend the GraphQL selections in `PR_DETAIL_QUERY`, `PR_COMMENTS_QUERY`, `PR_TIMELINE_QUERY`, and `DISCOVERY_QUERY` to include `avatarUrl` on every `author { ... }` / `actor { ... }` / `requestedReviewer ... on User` branch. The REST timeline payload (`/issues/{n}/timeline`) already carries `actor.avatar_url`; persist it through to the worker. The lazy hydrator (`fetch_pr_conversation`) also UPSERTs every comment + issue-comment author's avatar so a drawer open without a fresh sync still primes the cache.

**Read.** Every read-side DTO that surfaces a login grows an `avatar_url: Option<String>` (Rust) / `avatar_url: string | null` (TypeScript) sibling field, resolved via a `LEFT JOIN users ON users.login = ...login` at query time. The DTOs touched:

- `dashboard::DashboardPullRequest` — gains `author_avatar_url`.
- `dashboard::ReviewerEntry` — gains `avatar_url`.
- `conversation::ThreadHeadComment` — gains `avatar_url`.
- `conversation::ThreadComment` — gains `avatar_url`.
- `conversation::IssueComment` — gains `avatar_url`.
- `conversation::PullRequestReview` — gains `avatar_url`.
- `conversation::TimelineEventRecord` — gains `actor_avatar_url`.

**Frontend.** A new `PRismAvatar` primitive (`src/components/ui/PRismAvatar.vue`) renders `<img src=avatarUrl>` when the URL is present and falls back to the existing initials-in-coloured-circle pattern on `null` URL or `<img>` `onerror`. The pattern is centralised: every existing call site (`PullRequestRow`, `ReviewerStack`, `ThreadsList` head comment, `ReviewsTab`, `StatusTimelineTab` actor) routes through `PRismAvatar`. `initials` + `avatarSeed` (in `src/lib/format.ts`) stay — they back the fallback render.

## Consequences

### Positive

- The dashboard and conversation surfaces render real GitHub avatars across every author column, with deterministic initials when a row is fresh / offline / served by a stale URL.
- One row per login removes the duplication that a per-table column scheme creates. Avatar URL drift on GitHub propagates with one UPSERT.
- The cycle's network envelope stays the same: every author-bearing GraphQL selection grows by ~50 bytes (`avatarUrl` field), not by a new round-trip.
- The `LEFT JOIN users` pattern is uniform across dashboard + conversation queries, making future fields (`name`, `bio`, etc.) trivial to add on the same join.

### Negative

- A login that's never been seen by any sync cycle surfaces `avatar_url = None` and renders as initials. The first cycle that touches that login populates the cache; the second render lands the real image. Documented; acceptable for v1.
- Avatar URLs in `users` are not pruned with the PR they were first seen on — a login that authored a PR that's since been deleted lingers as a small row. The TEXT URL is ~80–100 bytes; even with 10k unique logins the table sits under 1 MiB. M5+ may revisit if multi-account scaling makes the table large enough to matter.
- The `<img>` element places a CDN request per first render of each avatar. The webview's HTTP cache eliminates the repeat cost; cold start hits the CDN once per login.

### Neutral / follow-ups

- The `users` table is intentionally account-agnostic: one row per login regardless of which account first observed them. Multi-account users see the same avatar for `@alice` across every account's dashboard slice — which is the correct behaviour because GitHub's `@alice` is the same person regardless of which token observed her.
- `last_seen_at` is written on every UPSERT so a future eviction policy can drop logins not seen in N days. v1 keeps every row.
- No avatar bytes are stored. The webview's HTTP cache handles image caching via GitHub's CDN response headers. If a future offline-first goal needs local bytes, an `avatars_blob(login PK, bytes, fetched_at)` table layers on top without touching the URL column.

## References

- ADR [0003](0003-local-storage-sqlite.md) — local SQLite storage.
- ADR [0006](0006-graphql-first-rest-fallback.md) — GraphQL-first protocol.
- ADR [0009](0009-pull-request-discovery-via-search-api.md) — discovery query that primes the cache for newly-discovered authors.
- ADR [0010](0010-conversation-depth-storage.md) — conversation-depth storage that the avatar join layers on top of.
- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md) — DTO + JOIN additions documented inline.
