# Architectural Decision Records

Decisions that shape the system live here. The process is in [CONTRIBUTING.md](../../CONTRIBUTING.md#adr-process).

Each ADR follows the [MADR](https://adr.github.io/madr/)-style template at [`0000-template.md`](0000-template.md), is named `NNNN-kebab-title.md` (sequence never re-used), and links the GitHub issue that authorised it.

## Index

| # | Title | Status | Issue |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted | — |
| [0002](0002-stack-tauri-vue-typescript.md) | App stack: Tauri 2 + Vue 3 + TypeScript | Accepted | [#1](https://github.com/cerinoligutom/PRism/issues/1) |
| [0003](0003-local-storage-sqlite.md) | Local storage: embedded SQLite | Accepted | [#2](https://github.com/cerinoligutom/PRism/issues/2) |
| [0004](0004-sync-polling-with-etag.md) | Sync strategy: polling with ETag / conditional requests | Accepted | [#3](https://github.com/cerinoligutom/PRism/issues/3) |
| [0005](0005-pat-auth-and-keychain-storage.md) | Authentication: PAT-only stored in OS keychain | Accepted | [#4](https://github.com/cerinoligutom/PRism/issues/4) |
| [0006](0006-graphql-first-rest-fallback.md) | GitHub API: GraphQL-first with REST fallback | Accepted | [#5](https://github.com/cerinoligutom/PRism/issues/5) |
| [0007](0007-status-timeline-from-timeline-events-api.md) | Status timeline derived from the timeline events API | Accepted | [#6](https://github.com/cerinoligutom/PRism/issues/6) |
| [0008](0008-wiki-auto-deploy.md) | Auto-deploy `docs/wiki/` to the GitHub wiki via Actions | Accepted | [#16](https://github.com/cerinoligutom/PRism/issues/16) |
| [0009](0009-pull-request-discovery-via-search-api.md) | Pull-request discovery via GitHub Search API | Accepted | [#35](https://github.com/cerinoligutom/PRism/issues/35) |
| [0010](0010-conversation-depth-storage.md) | Conversation-depth storage and hydration | Accepted | [#68](https://github.com/cerinoligutom/PRism/issues/68) |
| [0011](0011-cancel-inline-pr-detail-surface.md) | Cancel inline expansion as a third PR detail surface | Accepted | [#88](https://github.com/cerinoligutom/PRism/issues/88) |

## Statuses

- **Proposed** — under discussion, no commitment.
- **Accepted** — agreed and in effect.
- **Superseded by NNNN** — replaced by a later ADR (link the successor).
- **Deprecated** — no longer in effect, no replacement.
