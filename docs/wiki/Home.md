# PRism

Cross-platform desktop dashboard for managing GitHub pull requests.

PRism gives developers, reviewers, and engineering leads a single focused dashboard for every PR they care about — across repos, orgs, and GitHub accounts — with deeper visibility into conversation state, status timelines, and review signals than GitHub's native UI provides. V1 is **read-only**: PRism observes and surfaces state; review, comment, and merge actions happen on GitHub itself via a one-click "Open in browser" jump.

## Where to find things

- **[Getting Started](Getting-Started)** — prerequisites and dev environment setup.
- **[Architecture](Architecture)** — system design, with links to the ADRs that locked each decision.
- **[Roadmap](Roadmap)** — M1–M7 milestones and what's in each.
- **[Conventions](Conventions)** — quick reference for commits, PRs, ADRs.
- **[Platform QA](Platform-QA)** — per-platform checklist run before tagging a release.
- **[Releasing](Releasing)** — one-time setup + the per-release playbook (prepare → review → publish → manifest).
- **[FAQ](FAQ)** — common questions.

## Outside the wiki

- Roadmap board: <https://github.com/users/cerinoligutom/projects/7>
- Issues: <https://github.com/cerinoligutom/PRism/issues>
- ADRs (source): [docs/adr/](https://github.com/cerinoligutom/PRism/tree/main/docs/adr)
- Contributing: [CONTRIBUTING.md](https://github.com/cerinoligutom/PRism/blob/main/CONTRIBUTING.md)

## Status

Implementation feature-complete: M1-M7 plus the auto-update and in-app changelog wave are all on `main`. The release pipeline (prepare-release / release.yml / updater manifest) is wired end-to-end and the [Releasing](Releasing) playbook documents the per-release flow.

The v1.0.0 cut is currently **on hold** pending a deferred-polish sweep covering items carried over from M3-M6 (notifications polish, archive polish, internal refactors, and a few smaller surfaces). See [Roadmap → Pre-v1 polish](Roadmap#pre-v1-polish-current-focus) for the active list.

## Wiki source

This wiki is mirrored from [`docs/wiki/`](https://github.com/cerinoligutom/PRism/tree/main/docs/wiki) in the main repo. Edit there and open a PR; don't edit the wiki directly. The sync command is in [CONTRIBUTING.md](https://github.com/cerinoligutom/PRism/blob/main/CONTRIBUTING.md#wiki).
