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

`ui`, `sync`, `db`, `auth`, `tauri`, `github`, `notif`, `settings`, `docs`, `ci`, `adr`, `repo`, `release`. Use the scope that maps to the issue label and the area of code being touched. If a change spans many areas, omit the scope. `release` is reserved for the `chore(release): vX.Y.Z` PR opened by `.github/workflows/prepare-release.yml`.

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
4. Open a PR. The PR title is a Conventional Commit (e.g. `feat(github): GraphQL query for review thread resolution`). Set the assignee and apply labels at creation time — see [PR assignees and labels](#pr-assignees-and-labels) below.
5. Fill in every section of the [PR template](.github/PULL_REQUEST_TEMPLATE.md). Empty test plans get bounced.
6. If the change makes a non-trivial decision, add the ADR in the same PR. Link it from the PR description.
7. Resolve all review threads before merging.
8. Merge via **Squash and merge**. The PR title and body become the squash commit message — no further editing needed.

`main` requires squash merges, linear history, conversation resolution, and no force pushes. Direct pushes to `main` are forbidden.

### PR assignees and labels

Every PR — whether opened by a human or an AI agent — must set its assignee and apply the right labels **at creation time**, not after. This applies to `gh pr create`, the GitHub web UI, or any agent-driven flow.

- **Assignee:** the PR opener (`--assignee @me` on the CLI).
- **`type:*` label(s):** at minimum the one matching the Conventional Commit prefix in the title. Add a second `type:*` if the PR body's "Type of change" checklist ticks more than one (for example a `ci(...)` PR that also lands an ADR + CONTRIBUTING update is `type:ci` + `type:docs`).
- **`scope:*` label:** if the work cleanly maps to one of the seeded scopes (`scope:ui`, `scope:sync`, `scope:db`, `scope:auth`, `scope:tauri`, `scope:github`, `scope:notif`, `scope:settings`). Cross-cutting or docs-only PRs skip the scope label.
- **`priority:*` label:** propagated from the highest-priority linked issue, if any. PRs without a priority-bearing linked issue skip this label.

Worked example: a PR titled `feat(auth): persist PAT in keychain` that closes a `priority:p1` issue is opened with

```bash
gh pr create --assignee @me \
  --label "type:feat,scope:auth,priority:p1" \
  --title "feat(auth): persist PAT in keychain" \
  --body "<...>"
```

Why at creation time and not after: applying labels post-create can fire project-board workflows out of order (the "Item added to project" workflow sees an unlabelled item first), and assignees drive the board's "My items" view from the moment the PR exists.

## Changelog

PRism keeps a [Keep-a-Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) file at [`CHANGELOG.md`](CHANGELOG.md). If your PR introduces a user-facing change (new functionality, a behaviour change, a bug fix worth surfacing, a deprecation, a removed surface, or a security-relevant fix), append a one-line bullet to the matching subheading under `[Unreleased]` in the same PR. Internal refactors, build / CI plumbing, and docs-only changes don't need an entry. Releases promote `[Unreleased]` to a dated version block via `pnpm stamp-changelog --version X.Y.Z`; don't add dated version headings by hand.

## Release pipeline

Releases are cut by two coupled workflows in [`.github/workflows/`](.github/workflows/) (see ADR-0023 for the design):

1. **`prepare-release.yml`** runs on manual `workflow_dispatch`. Pick the SemVer bump (`patch` / `minor` / `major`) or supply an explicit `version` override; an optional `dry_run` boolean previews the composed notes in the workflow summary without opening a PR. On a real run the workflow bumps the three version files via `pnpm bump-version`, promotes `[Unreleased]` via `pnpm stamp-changelog`, composes release notes from squashed PR titles since the last `v*` tag (grouped by Conventional Commit type), and opens a `chore(release): vX.Y.Z` PR on a `release/vX.Y.Z` branch.
2. **`tag-on-release-merge.yml`** watches `push: main` for the shape `prepare-release.yml` produces (subject `chore(release): vX.Y.Z (#NNN)` + diff touching `CHANGELOG.md` + `package.json` + `src-tauri/Cargo.toml`). On match it reads the version from `package.json` and pushes a matching `vX.Y.Z` tag at the merge commit. Ordinary `chore(deps): ...` commits and one-file CHANGELOG nudges are filtered out by the subject regex + the touched-files check.

The tag push downstream trips the cross-platform build workflow (a separate file, tracked in #304). Re-running `prepare-release.yml` for the same version is blocked by the "refuse to overwrite an existing tag" guard.

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

Authoritative wiki source lives in [`docs/wiki/`](docs/wiki/) so changes go through PR review. The published wiki at <https://github.com/cerinoligutom/PRism/wiki> is **mirrored automatically** by [`.github/workflows/wiki-sync.yml`](.github/workflows/wiki-sync.yml) on every push to `main` that touches `docs/wiki/`. The workflow can also be triggered manually from the Actions tab (`workflow_dispatch`).

Don't edit the published wiki directly — the next sync overwrites it. Edit `docs/wiki/`, open a PR, and the sync runs on merge.

### One-time setup

The workflow needs two prerequisites; they only have to be done once.

1. **Initialise the wiki repo.** GitHub doesn't create the wiki's underlying git repo until a first page exists. Visit <https://github.com/cerinoligutom/PRism/wiki> and create any placeholder page through the UI — the next sync overwrites it.
2. **Create the `WIKI_TOKEN` secret.** Generate a fine-grained PAT at <https://github.com/settings/personal-access-tokens/new> scoped only to this repository, with **Repository permissions → Contents: Read and write**. Set an expiry that fits your rotation cadence. Add it as a repo secret named `WIKI_TOKEN` at <https://github.com/cerinoligutom/PRism/settings/secrets/actions>.

### Wiki page names

GitHub turns hyphens into spaces, so `Getting-Started.md` becomes the page **Getting Started**.

### Manual fallback

If the workflow is offline (token expired, wiki repo wedged), a one-shot manual sync still works:

```bash
SHA=$(git rev-parse --short HEAD)
git clone git@github.com:cerinoligutom/PRism.wiki.git /tmp/PRism.wiki
rsync -av --delete --exclude='.git' docs/wiki/ /tmp/PRism.wiki/
cd /tmp/PRism.wiki
git add -A
git commit -m "sync from docs/wiki@${SHA}"
git push
```

## Updater manifest

The Tauri updater consumes a `latest.json` manifest published on GitHub Pages. ADR-0024 owns the design; [`.github/workflows/update-manifest.yml`](.github/workflows/update-manifest.yml) regenerates the manifest whenever a draft Release is published (gate 3 of ADR-0023's review chain).

### One-time setup

After the workflow lands its first commit on the `gh-pages` branch (which it creates automatically on the first matching `release: published` event), set repo **Settings -> Pages -> Source** to "Deploy from a branch" with **Branch: `gh-pages` / Root (`/`)**. GitHub Pages then serves `latest.json` at `https://cerinoligutom.github.io/PRism/latest.json`. The updater plugin's `endpoints` value in `src-tauri/tauri.conf.json` (added by issue #308) points at that URL.

No new secrets are needed: the workflow signs nothing of its own. It reads the `.sig` files that `release.yml` produced via `TAURI_SIGNING_PRIVATE_KEY` and copies the signatures into the manifest verbatim.

### Updater signing key

The Tauri updater verifies every downloaded artefact against a public key embedded in `src-tauri/tauri.conf.json`. The matching private key signs each release's `.app.tar.gz` / `.AppImage` / `.exe` via `release.yml`. Both are generated once by the maintainer:

1. Generate the keypair locally with the Tauri CLI:

   ```bash
   pnpm tauri signer generate -w ~/.tauri/prism-updater.key
   ```

   The command prints the public key to stdout and writes the password-protected private key to the path passed via `-w`.

2. Add the private key as the `TAURI_SIGNING_PRIVATE_KEY` GitHub Actions secret (`Settings -> Secrets and variables -> Actions -> New repository secret`). Paste the file contents verbatim.

3. Add the password as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

4. Replace the `REPLACE_WITH_GENERATED_PUBLIC_KEY` placeholder in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`) with the public key printed in step 1. Commit the change.

5. Confirm the GH Pages source setting from the "Updater manifest" section above so the published `latest.json` is reachable at `https://cerinoligutom.github.io/PRism/latest.json`.

The private key never leaves the maintainer's machine or the GitHub Actions secret store. Rotating it means re-running `tauri signer generate`, updating both secrets, and shipping a release whose `pubkey` matches the new private key.

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
