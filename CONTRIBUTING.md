# Contributing to PRism

Thanks for working on PRism. This document is the source of truth for how the project is developed: commit style, branch and PR workflow, the ADR process, and how the wiki is kept in sync.

## TL;DR

- **Commits:** [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).
- **Branches:** `<type>/<short-kebab-summary>` (e.g. `feat/keychain-storage`, `fix/etag-edge-case`).
- **PRs:** open against `main`, follow the [pull request template](.github/PULL_REQUEST_TEMPLATE.md), get merged via **squash** only.
- **Decisions:** anything non-trivial gets an [ADR](docs/adr/) tied to a GitHub issue.
- **Issues:** start from one of the [issue templates](.github/ISSUE_TEMPLATE), land on the [kanban board](https://github.com/users/cerinoligutom/projects/7).

## Conventional Commits

We follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/). Every commit message — and crucially, every **PR title** (because we squash-merge, the PR title becomes the commit message on `main`) — must take the form:

```
<type>(<scope>)!: <description>

<body>

<footer>
```

### Types

| Type | Use for |
|---|---|
| `feat` | New user-visible functionality |
| `fix` | Bug fix |
| `docs` | Documentation only (README, wiki, ADRs, comments) |
| `style` | Whitespace / formatting only — no logic change |
| `refactor` | Code change that doesn't alter behaviour |
| `perf` | Performance improvement |
| `test` | Tests only |
| `build` | Build system, packaging, dependency updates |
| `ci` | CI configuration |
| `chore` | Anything else (repo plumbing, housekeeping) |
| `revert` | Reverts a previous commit; body must reference the reverted SHA |

### Scopes (optional but encouraged)

`ui`, `sync`, `db`, `auth`, `tauri`, `github`, `notif`, `settings`, `docs`, `ci`. Use the scope that maps to the issue label and the area of code being touched. If a change spans many areas, omit the scope.

### Breaking changes

A `!` after the type/scope marks a breaking change, and the footer must include `BREAKING CHANGE: <description>`. Example:

```
feat(auth)!: drop classic PAT support

BREAKING CHANGE: existing classic PATs will be invalidated on first launch; users must re-add a fine-grained PAT.
```

### Linking issues

Reference the issue in the footer: `Closes #N`, `Fixes #N`, or `Refs #N`. A PR that closes an issue must include `Closes #N` so GitHub auto-closes it on merge.

### Examples

```
feat(github): GraphQL query for review thread resolution
fix(sync): handle 304 with no body on conditional GET
docs(adr): record decision to use SQLite for local cache
chore(repo): seed labels and milestones
build(tauri): bump tauri to 2.0.0
```

## Branch naming

`<type>/<short-kebab-summary>`. The `<type>` is the Conventional Commit type. The summary is 2–5 words. Examples: `feat/auth-keychain`, `fix/timeline-event-ordering`, `docs/wiki-architecture`.

`main` is the only long-lived branch. Feature branches are deleted automatically on merge.

## Pull request workflow

1. Open or claim an issue. If the work is non-trivial and isn't covered by an existing issue, open one first using the appropriate template — this keeps the kanban board honest.
2. Branch from `main`.
3. Make the change. Keep PRs focused and reviewable; if a slice is growing past ~400 lines of diff, split it.
4. Open a PR. The PR title is a Conventional Commit (e.g. `feat(github): GraphQL query for review thread resolution`).
5. Fill in every section of the [PR template](.github/PULL_REQUEST_TEMPLATE.md). Empty test plans get bounced.
6. If the change makes a non-trivial decision, add the ADR in the same PR. Link it from the PR description.
7. Resolve all review threads before merging.
8. Merge via **Squash and merge**. The PR title and body become the squash commit message — no further editing needed.

`main` requires squash merges, linear history, conversation resolution, and no force pushes. Direct pushes to `main` are forbidden.

## ADR process

Anything that changes the shape of the system gets an ADR. Stack choice, storage engine, sync model, security model, API protocol — all ADR-worthy. Library upgrades, bug fixes, and routine refactors are not.

- ADRs live in [`docs/adr/`](docs/adr/).
- File names: `NNNN-kebab-title.md` (four-digit zero-padded sequence, never re-used).
- Use the [template](docs/adr/0000-template.md). Always link the GitHub issue that authorised the decision.
- Status values: **Proposed**, **Accepted**, **Superseded by NNNN**, **Deprecated**.
- The [ADR index](docs/adr/README.md) is updated in the same PR.

Workflow:

1. Open an issue labelled `needs-adr` describing the decision to be made.
2. Open a PR that adds the ADR (status: **Proposed**). The PR title is `docs(adr): <decision>`.
3. Once approved, change status to **Accepted** in the same PR or a follow-up before merging.
4. Reference the ADR from CLAUDE.md, the wiki Architecture page, and any code comment where the decision is non-obvious.

To supersede an ADR, write a new one and set the old one's status to `Superseded by NNNN`. Never delete or rewrite an accepted ADR — the audit trail matters more than tidiness.

## Wiki

Authoritative wiki source lives in [`docs/wiki/`](docs/wiki/) so changes go through PR review. The published wiki at <https://github.com/cerinoligutom/PRism/wiki> is mirrored from there.

To mirror after a PR lands:

```bash
git clone git@github.com:cerinoligutom/PRism.wiki.git /tmp/PRism.wiki
cp docs/wiki/*.md /tmp/PRism.wiki/
cd /tmp/PRism.wiki
git add -A
git commit -m "sync from docs/wiki@<short-sha>"
git push
```

Wiki page names follow GitHub's convention: hyphens become spaces in titles, so `Getting-Started.md` becomes the page **Getting Started**.

## Issues

- Pick the template that fits: bug, feature request, or chore.
- Add a `type:*` label and a `scope:*` label.
- Assign a milestone if the work clearly slots into M1–M7.
- After creating, drop the issue onto the [kanban board](https://github.com/users/cerinoligutom/projects/7).

## Code style

Linters and formatters land with the Tauri scaffold (issue #8). Until then, follow conventional defaults:

- Rust: `rustfmt` defaults, `clippy` warnings treated as errors.
- TypeScript: `tsc --noEmit` clean; ESLint + Prettier with defaults; `strict: true` in `tsconfig.json`.
- Markdown: no smart quotes, no em/en-dashes; use plain `-` and `"`. One sentence per paragraph is fine; don't split sentences across lines.

## Communication

- Architecture & decisions: ADRs.
- How-to & onboarding: wiki.
- Roadmap & milestones: GitHub milestones + the [kanban board](https://github.com/users/cerinoligutom/projects/7).
- Bugs & feature ideas: GitHub issues.
