# Architecture

This page is the durable source of truth for PRism's system design. Each load-bearing decision is summarised here and linked to its ADR in `docs/adr/`.

## Top-down view

```
┌────────────────────────────────────────────────────────────────────┐
│                            PRism (desktop)                         │
│                                                                    │
│  ┌─────────────────────┐        ┌─────────────────────────────┐    │
│  │  Vue 3 frontend     │        │  Rust core (Tauri 2)        │    │
│  │  (TS / Vite / Reka  │  IPC   │  - sync worker (per acct)   │    │
│  │   UI / Tailwind /   │ <────> │  - GitHub clients           │    │
│  │   Pinia / VueUse)   │        │     - GraphQL (primary)     │    │
│  │                     │        │     - REST (fallback)       │    │
│  │  Reads from SQLite, │        │  - storage (SQLite)         │    │
│  │  renders dashboard. │        │  - keychain (PAT storage)   │    │
│  └─────────────────────┘        │  - notifications (native)   │    │
│                                 └─────────────────────────────┘    │
│                                          │           │              │
│                                          ▼           ▼              │
│                                       SQLite      OS keychain       │
└────────────────────────────────────────────────────────────────────┘
                                          │
                                          ▼
                                  GitHub API
                            (GraphQL + REST, via HTTPS,
                             with ETag conditional reqs)
```

## Stack

- **Tauri 2** core (Rust + system webview) — ADR [0002](../../docs/adr/0002-stack-tauri-vue-typescript.md).
- **Vue 3 + TypeScript** frontend via Vite, with Reka UI (headless), Tailwind, Pinia, Vue Router (where multi-view), VueUse — ADR 0002.
- **SQLite** for the local cache, via `rusqlite` from the Rust core — ADR [0003](../../docs/adr/0003-local-storage-sqlite.md).
- **OS keychain** for PAT storage via Tauri secure storage APIs — ADR [0005](../../docs/adr/0005-pat-auth-and-keychain-storage.md).
- **GraphQL-first** GitHub client, with REST only for endpoints GraphQL doesn't cover — ADR [0006](../../docs/adr/0006-graphql-first-rest-fallback.md).

## Data flow

1. **Auth.** User adds a labelled PAT per account (PRD §5.1). The token goes to the OS keychain; non-secret metadata (label, host, login, scopes) goes to a small encrypted local config. Validation hits `GET /user` before storing. See ADR 0005.
2. **Sync.** A Rust worker polls per account on the configured interval (default 60s, range 30s–10min). Each poll uses ETag / `If-Modified-Since` conditional requests; per-resource ETags live in SQLite. Failures and rate-limit hits are isolated per account. See ADR [0004](../../docs/adr/0004-sync-polling-with-etag.md).
3. **Storage.** PRs, reviews, review threads, review comments, issue comments, timeline events, check runs, repos, and accounts live in SQLite. The cache is single-writer (sync worker) / multi-reader (UI). See ADR 0003.
4. **Status reconstruction.** "Latest status change" is not a native GitHub field. It's derived from the REST timeline events API, picking the most recent of `ready_for_review`, `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`, `reopened`. See ADR [0007](../../docs/adr/0007-status-timeline-from-timeline-events-api.md).
5. **Conversation depth.** Resolved-thread state requires GraphQL (`pullRequestReviewThreads.isResolved`). Conversation stats (oldest unresolved thread age, average time-to-response, resolution rate) are computed incrementally per-thread and cached, not recomputed every sync.
6. **UI.** Vue components read from SQLite on demand and re-render when the worker emits change events through Tauri's IPC. The dashboard never queries GitHub directly.

## Views

Four built-in views, switchable from the sidebar (PRD §5.2):

- **Authored by me**
- **Assigned to me as reviewer** — split into "needs first review" and "needs re-review"
- **Watching / participated** — auto-tracked involvement
- **Team / org-wide** — per-repo opt-in for rate-limit safety

Each view supports grouping (org, repo, org→repo nested, or flat), sorting (newest / oldest / staleness / comment count / composite "needs my attention"), and quick-filter chips (PRD §5.4).

## Auto-tracking

Any PR the user touches (author, assignee, reviewer, commenter, mentionee, reactor, subscriber) is automatically tracked and appears in the "Watching / participated" view (PRD §5.5).

- 30-day inactivity TTL: closed/merged PRs auto-archive after 30 days inactive.
- Open PRs go stale visually after 30 days but stay visible behind a "Stale" filter chip.
- Closed/merged retention is 14 days by default (configurable) before archive.

## Notifications

Both desktop (native OS toasts) and in-app (badges). In-app badges are the default; toasts are per-event opt-in. Quiet hours suppress toasts. See PRD §5.6.

## Non-functional targets

| Target | Value | Source |
|---|---|---|
| Cold start → first paint | < 2 s | PRD §8.1 |
| Render 500 PRs across 50 repos | < 500 ms | PRD §8.1 |
| Memory with 500 PRs cached | < 200 MB | PRD §8.1 |
| Binary size per platform | < 20 MB | PRD §8.1 |
| API budget per account | < 20% of 5000 req/hr | PRD §8.2 |
| 95th-percentile freshness | < 2 min stale | PRD §8.3, ADR 0004 |

## Out of scope for v1

Write actions, inline diff viewing, non-GitHub platforms, AI features, mobile/web, team analytics, custom automation, shared team views, webhook real-time updates, OAuth or GitHub App auth. See [Roadmap](Roadmap) for what's deferred to post-v1.

## Decision records

See the [ADR index](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/README.md) for the full list. Active ADRs:

| # | Title |
|---|---|
| 0001 | Record architecture decisions |
| 0002 | App stack: Tauri 2 + Vue 3 + TypeScript |
| 0003 | Local storage: embedded SQLite |
| 0004 | Sync strategy: polling with ETag / conditional requests |
| 0005 | Authentication: PAT-only stored in OS keychain |
| 0006 | GitHub API: GraphQL-first with REST fallback |
| 0007 | Status timeline derived from the timeline events API |
