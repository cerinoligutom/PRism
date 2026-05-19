# 0008 — Auto-deploy `docs/wiki/` to the GitHub wiki via Actions

- **Status:** Accepted
- **Date:** 2026-05-20
- **Issue:** [#16](https://github.com/cerinoligutom/PRism/issues/16)
- **Deciders:** @cerinoligutom

## Context

Wiki source lives in [`docs/wiki/`](../wiki/) (per ADR [0001](0001-record-architecture-decisions.md) and CONTRIBUTING.md) so changes go through PR review. The published wiki at <https://github.com/cerinoligutom/PRism/wiki> is a separate git repository (`PRism.wiki.git`); without automation, every wiki update needs a manual clone + copy + push, which drifts the moment someone forgets.

GitHub doesn't expose wiki settings to its native auto-publish or branch-protection plumbing. Synchronising the two repos is something we have to build, not something we can configure.

The wiki repo is also a special case for GitHub auth: the default `GITHUB_TOKEN` granted to Actions runs does **not** have wiki write access. Wikis require either an SSH deploy key or a Personal Access Token (PAT) with `repo` scope (or fine-grained `Contents` write on the parent repo, which transitively covers the wiki).

## Decision drivers

- Lock-step parity between in-repo source and the published wiki, without operator overhead.
- Minimal supply-chain surface — every third-party action is a privileged dependency.
- No new infrastructure to host or maintain.
- Auth path that's easy to rotate and clearly scoped.

## Considered options

1. **Manual sync (status quo)** — clone, copy, push by hand. Documented in CONTRIBUTING.md but easily forgotten.
2. **Third-party Action** (e.g. `Andrew-Chen-Wang/github-wiki-action`) — ~5 lines of YAML, but pins us to an external action's behaviour, security posture, and pace of updates.
3. **Hand-rolled workflow** (this ADR) — ~30 lines of YAML using `actions/checkout` and shell `git` commands. Pins on first-party actions only.
4. **Git submodule / subtree** — `docs/wiki/` as a subtree of `PRism.wiki.git`. Rejected: noisy git history, bad PR ergonomics, requires every contributor to learn subtree mechanics.

## Decision

We will add **a hand-rolled GitHub Actions workflow** at `.github/workflows/wiki-sync.yml` that runs on `push` to `main` filtered to `docs/wiki/**` (and on `workflow_dispatch`). The workflow uses only first-party `actions/checkout` plus shell `git`, and authenticates to the wiki repo via a repo secret `WIKI_TOKEN` — a fine-grained PAT scoped to this repo with **Contents: Read and write** (which covers the wiki).

The workflow:

1. Checks out the main repo (sparse — only `docs/wiki/`).
2. Checks out the wiki repo via the PAT.
3. `rsync`s `docs/wiki/` over the wiki working tree (deletes pages no longer in source, preserves `.git`).
4. Commits and pushes only when the diff is non-empty.

A no-op when the wiki content already matches is cheap and explicit.

## Consequences

### Positive

- Wiki always reflects the most recent `main` commit that touched `docs/wiki/`.
- Single direction of truth: edits land in PRs against the main repo, never directly on the wiki.
- No third-party action in the supply chain.
- Workflow is small enough to audit at a glance.

### Negative

- A repo secret (`WIKI_TOKEN`) must be created and rotated periodically. Token expiry surfaces as a workflow failure rather than silent drift, which is the right failure mode.
- A direct edit on the published wiki via the GitHub UI is overwritten on the next sync. Acceptable — we explicitly want source-of-truth in the repo.
- Initial setup needs the wiki repo to exist, which means a human creates the first page through the GitHub UI before the workflow can run. One-time cost.

### Neutral / follow-ups

- If we ever need image attachments uploaded through the GitHub UI to survive, the sync logic would need to be adjusted (currently `rsync --delete` removes everything not in `docs/wiki/`). Until then, images live in `docs/wiki/` alongside the markdown.
- A future ADR may codify a broader CI policy (when to use third-party actions vs hand-rolled, version-pinning policy).

## References

- [GitHub Actions: `actions/checkout`](https://github.com/actions/checkout)
- [GitHub wiki access via PAT](https://docs.github.com/en/communities/documenting-your-project-with-wikis/about-wikis)
- ADR [0001](0001-record-architecture-decisions.md) — established the in-repo wiki source model.
