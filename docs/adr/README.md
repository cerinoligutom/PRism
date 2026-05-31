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
| [0012](0012-threads-bar-four-state-and-outdated-counted.md) | Threads-bar four-state redesign and outdated counted in the denominator | Accepted | [#98](https://github.com/cerinoligutom/PRism/issues/98) |
| [0013](0013-user-avatars-cache.md) | User avatar caching via a `users` table | Accepted | [#99](https://github.com/cerinoligutom/PRism/issues/99) |
| [0014](0014-comment-markdown-rendering.md) | Comment markdown rendering via GitHub `bodyHTML` + Shiki client highlighting | Accepted | [#138](https://github.com/cerinoligutom/PRism/issues/138) |
| [0015](0015-triage-state-model.md) | Triage state model: per-account read-state, mention detection, and "needs my attention" composite | Accepted (0031 in part) | [#144](https://github.com/cerinoligutom/PRism/issues/144) |
| [0016](0016-unified-multi-account-dashboard.md) | Unified multi-account dashboard: dedupe-and-merge, query-time threads rollup, per-account failure isolation | Accepted | [#163](https://github.com/cerinoligutom/PRism/issues/163) |
| [0017](0017-desktop-notifications.md) | Desktop notifications: triggers, app-wide preferences, macOS-only dock badge, deferred permission prompt | Accepted (0031 in part) | [#188](https://github.com/cerinoligutom/PRism/issues/188) |
| [0018](0018-archive-and-ttl.md) | Archive bucket: per-(account, PR) `archived_at`, 30-day inactivity TTL, manual + auto, reversible | Accepted | [#189](https://github.com/cerinoligutom/PRism/issues/189) |
| [0019](0019-error-handling-and-reauth-flow.md) | Error handling: per-store `lastError`, no toast for failures, self-healing reauth | Accepted | [#287](https://github.com/cerinoligutom/PRism/issues/287) |
| [0020](0020-settings-persistence-boundary.md) | Settings persistence: SQLite for worker-visible state, localStorage for device-local UI prefs | Accepted | [#287](https://github.com/cerinoligutom/PRism/issues/287) |
| [0021](0021-rust-to-typescript-type-bindings.md) | Rust to TypeScript type bindings: stay manual through v1 | Accepted | [#293](https://github.com/cerinoligutom/PRism/issues/293) |
| [0022](0022-versioning-and-build-metadata.md) | Versioning scheme and build-metadata pipeline | Accepted | [#295](https://github.com/cerinoligutom/PRism/issues/295) |
| [0023](0023-release-pipeline.md) | Release pipeline: two-workflow split, draft GitHub Release, unsigned at OS level | Accepted | [#297](https://github.com/cerinoligutom/PRism/issues/297) |
| [0024](0024-auto-update-mechanism.md) | Auto-update mechanism: `tauri-plugin-updater`, opt-in default, GH Pages manifest, silent failure | Accepted | [#301](https://github.com/cerinoligutom/PRism/issues/301) |
| [0025](0025-in-app-changelog.md) | In-app changelog: bundled `CHANGELOG.md`, last-seen version gate, single concatenated dialog | Accepted | [#302](https://github.com/cerinoligutom/PRism/issues/302) |
| [0026](0026-tracing-adoption.md) | Backend logging via `tracing` to stdout | Accepted | [#334](https://github.com/cerinoligutom/PRism/issues/334) |
| [0027](0027-timeline-event-expansion.md) | Expanding the StatusTimelineTab event set | Accepted | [#342](https://github.com/cerinoligutom/PRism/issues/342) |
| [0028](0028-persistent-notifications-inbox.md) | Persistent notifications inbox | Accepted (0031 in part) | [#378](https://github.com/cerinoligutom/PRism/issues/378) |
| [0029](0029-sync-owns-conversation-persistence.md) | Sync owns conversation persistence (supersedes 0010's lazy-hydration decision) | Accepted | [#422](https://github.com/cerinoligutom/PRism/issues/422) |
| [0030](0030-ci-checks-json-column.md) | Per-check CI detail stored as a denormalised JSON column | Accepted | [#426](https://github.com/cerinoligutom/PRism/issues/426) |
| [0031](0031-conversation-unit-attention-and-rearm-dispatch.md) | Conversation-unit attention model, derived inbox read-state, and edge-with-re-arm dispatch | Accepted | [#428](https://github.com/cerinoligutom/PRism/issues/428) |
| [0032](0032-prune-pre-0031-notification-remnants.md) | Prune the pre-0031 notification remnants (`notify_on_mention`, `mentioned_count_unread`) | Accepted | [#451](https://github.com/cerinoligutom/PRism/issues/451) |

## Statuses

- **Proposed** — under discussion, no commitment.
- **Accepted** — agreed and in effect.
- **Superseded by NNNN** — replaced by a later ADR (link the successor).
- **Deprecated** — no longer in effect, no replacement.
