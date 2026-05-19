# Roadmap

PRism v1 is delivered across seven milestones. Each milestone is tracked in GitHub; the kanban board view of all milestones is at <https://github.com/users/cerinoligutom/projects/7>.

| Milestone | Focus |
|---|---|
| [M1 — Foundations](https://github.com/cerinoligutom/PRism/milestone/1) | Tauri scaffold, PAT auth + keychain storage, SQLite schema, single-account GitHub sync (REST + GraphQL clients) |
| [M2 — Core dashboard](https://github.com/cerinoligutom/PRism/milestone/2) | Four views (authored / reviewer / watching / team), grouping by org and repo, basic per-PR row (title, timing, reviewer state, CI summary) |
| [M3 — Conversation depth](https://github.com/cerinoligutom/PRism/milestone/3) | Per-thread state, comment-type breakdown, conversation stats (oldest unresolved, avg time-to-response, resolution rate), per-thread previews |
| [M4 — Triage UX](https://github.com/cerinoligutom/PRism/milestone/4) | Sorting, quick-filter chips, search, unread/mention highlighting, "needs my attention" composite signal |
| [M5 — Multi-account & GHE](https://github.com/cerinoligutom/PRism/milestone/5) | Multiple PATs, per-account host config, GHE compatibility testing |
| [M6 — Notifications & polish](https://github.com/cerinoligutom/PRism/milestone/6) | Desktop notifications, in-app badges, settings UI, last-synced indicator, manual refresh, archive bucket, TTL |
| [M7 — Hardening & launch](https://github.com/cerinoligutom/PRism/milestone/7) | Performance tuning, rate-limit guardrails, error handling, keychain edge cases, cross-platform QA |

Milestone order is indicative; M1 must land first, but the others overlap.

## Out of scope for v1

Deferred to post-v1; tracked separately when committed:

- Write actions (approve, comment, merge, request changes, resolve threads).
- Inline diff viewer.
- Non-GitHub platforms (GitLab, Bitbucket, Azure DevOps).
- AI features (review summaries, suggestion assist, auto-categorisation).
- Mobile or web companion.
- Team analytics dashboards.
- Custom automation rules / workflow triggers.
- Shared / synced team views.
- Webhook-driven real-time updates (would require a hosted relay).
- OAuth Device Flow or GitHub App auth.

## Possible post-v1 directions

Not commitments — see [Architecture](Architecture) for the v1 boundaries:

- OAuth Device Flow or GitHub App auth as PAT alternatives.
- Lightweight write actions (approve, comment, mark thread resolved).
- Inline diff viewer.
- Multi-platform expansion.
- Webhook-backed real-time updates via a hosted relay.
- AI summarisation, suggested review priorities, thread digests.
- Opt-in team analytics.
- Shared team views / synced filters.
- Mobile companion app.
