# AGENTS.md — Instructions for AI coding agents

PRism uses Claude Code as its primary coding assistant; the full ruleset is in [CLAUDE.md](CLAUDE.md). This file is the entry point for other agents (Codex, Cursor, Cline, OpenCode, Aider, Gemini CLI, Copilot Agent, etc.) and points you at the project-specific guardrails before you write a line of code.

## Read first

1. [README.md](README.md) — what PRism is.
2. [CLAUDE.md](CLAUDE.md) — the working agreements, style, and quality bar. They apply to you too.
3. [CONTRIBUTING.md](CONTRIBUTING.md) — Conventional Commits, branch and PR flow, ADR process.
4. The wiki [Architecture page](https://github.com/cerinoligutom/PRism/wiki/Architecture) — current system design with ADR links.
5. The relevant [ADRs](docs/adr/) for the area you're touching.

## Non-negotiables

- **Conventional Commits** on every commit and PR title. PRs are squash-merged, so the PR title becomes the commit on `main`.
- **Link to a GitHub issue** in every PR — `Closes #N` or `Refs #N`. If no issue exists for the work, open one first.
- **Write an ADR** for non-trivial decisions (stack, storage, sync, security, API protocol, library choice with downstream impact). Use [`docs/adr/0000-template.md`](docs/adr/0000-template.md).
- **Never commit secrets.** PATs live in the OS keychain via Tauri secure storage — nowhere else.
- **Australian English** in prose, comments, and identifiers.
- **Plain formatting** in markdown: no smart quotes, no em-dashes, no en-dashes.
- **Don't invent files.** Update existing docs; only create new markdown when the new document is itself the deliverable (an ADR, a wiki page).

## Tooling expectations

- Frontend: Vue 3 + TypeScript via Vite. `strict: true`, no `any`.
- Backend: Rust via Tauri 2. `rustfmt` clean, `clippy` warnings as errors.
- Storage: SQLite via the migration runner in `src-tauri/migrations/`. Parameterised SQL only.
- GitHub: GraphQL-first, REST only where GraphQL lacks coverage. ETag / conditional requests on every read.

## Quality bar before opening a PR

- Builds cleanly: `pnpm tauri build` (or the appropriate scoped command for the area you touched).
- Type-checks: `pnpm tsc --noEmit`.
- Lints / formatters pass (Rust + TS).
- Tests pass — new code includes tests where the change is non-trivial.
- Self-review pass: re-read your diff before requesting review.

## When in doubt

Open an issue with the `needs-triage` label describing the question. Don't guess at architecture, don't skip ADRs, don't ship placeholder code.
