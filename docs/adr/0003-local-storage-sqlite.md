# 0003 — Local storage: embedded SQLite

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#2](https://github.com/cerinoligutom/PRism/issues/2)
- **Deciders:** @cerinoligutom

## Context

PRism caches PR data locally: PRs, reviews, review threads, review comments, issue comments, timeline events, check runs, repos, and accounts (PRD §7.2). The cache must persist across launches, support relational queries (joins for the dashboard groupings), tolerate dirty shutdowns, and stay well under the 200 MB total app footprint with 500 PRs cached.

The cache is **single-writer** (the background sync worker) and **multi-reader** (the UI). It is never shared across machines.

## Decision drivers

- Relational query support (joins, indices, aggregates for conversation stats).
- Reliable on-disk persistence with crash safety.
- Small dependency footprint — no separate server process.
- Cross-platform behaviour (macOS / Windows / Linux).
- Embeddable in Tauri's Rust core without exotic build steps.
- Migration story for schema changes.

## Considered options

1. **SQLite** — embedded, relational, mature, single file on disk, well-supported in Rust (`rusqlite`, `sqlx`).
2. **sled** — embedded KV in Rust, modern, but pre-1.0 and not relational.
3. **redb** — embedded ACID KV in Rust, 1.0+, fast, but not relational.
4. **File-based JSON / TOML** — eliminated by the relational-query and crash-safety requirements.
5. **PostgreSQL / DuckDB embedded** — over-scaled for a single-user cache.

## Decision

We will use **embedded SQLite** as the local cache, accessed via `rusqlite` from the Rust core. Schema lives as numbered SQL migrations in `src-tauri/migrations/`; a migration runner executes them on app startup. Final choice of migration library (`refinery` / `sqlx::migrate!` / hand-rolled) is deferred to the schema implementation issue.

Rationale: SQLite is the only option in the list that gives us relational queries, mature crash safety (WAL mode), zero operational overhead, and a battle-tested Rust binding. The cache shape (PRs with reviews, threads, comments, events) is naturally relational; making it KV-shaped to fit a non-relational store would cost more than it saves.

## Consequences

### Positive

- Joins, aggregates, and indices for dashboard queries are trivial.
- Backups are a file copy.
- Schema migrations are a standard problem with standard solutions.
- Well-understood operational behaviour, even with surprise power-offs.

### Negative

- SQL hand-written or via a query builder; we accept the verbosity over an ORM's runtime surprises.
- Schema migrations need discipline (forward-only, never edit-in-place).

### Neutral / follow-ups

- A follow-up ADR may decide on a migration library after [#9](https://github.com/cerinoligutom/PRism/issues/9) lands.
- WAL mode and journal_size pragmas are tuning decisions for an issue, not an ADR.

## References

- [rusqlite](https://github.com/rusqlite/rusqlite)
- [refinery](https://github.com/rust-db/refinery)
- PRD §7.2, §8.1
