# 0001 — Record architecture decisions

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** —
- **Deciders:** @cerinoligutom

## Context

PRism is a single-developer project today, with the expectation of contributors and AI coding agents later. Decisions about stack, storage, sync, security, and API protocols compound — when a future maintainer (human or otherwise) asks _"why did we pick X?"_, the answer needs to be discoverable, dated, and tied to the constraints in effect at the time. Tribal knowledge in commit messages and chat logs decays fast; structured records survive.

## Decision drivers

- Auditability of non-trivial decisions.
- Onboarding cost for new contributors and agents.
- Ability to supersede a decision cleanly when context changes.
- Discoverability — decisions should be linkable from code, PRs, and the wiki.

## Considered options

1. **No formal process** — rely on commit messages and the wiki.
2. **Confluence / Notion / Google Docs** — host decisions outside the repo.
3. **In-repo ADRs (MADR-style)** — markdown files versioned alongside the code.

## Decision

We will keep [MADR-style](https://adr.github.io/madr/) ADRs in `docs/adr/`, named `NNNN-kebab-title.md` with a never-reused four-digit sequence, each linking the GitHub issue that authorised the decision.

Rationale: ADRs live next to the code they govern; they're reviewed via the same PR flow as code changes; they're greppable; and they require no extra service. The GitHub issue link gives every decision a discussion record without polluting the ADR with running commentary.

## Consequences

### Positive

- Decisions have permanent, dated, reviewable records.
- New contributors and agents can read `docs/adr/README.md` to understand the system's load-bearing choices.
- Superseding is a regular operation, not a special case.

### Negative

- Small overhead per decision: an extra markdown file and an index update.
- Authors must judge when a decision warrants an ADR. The rule of thumb in [CONTRIBUTING.md](../../CONTRIBUTING.md#adr-process) helps but isn't airtight.

### Neutral / follow-ups

- Index is maintained manually for now. If the count grows past ~40, automate generation.
- ADRs 0002–0007 follow from this one and lock in the PRD §7 decisions.

## References

- [MADR](https://adr.github.io/madr/)
- [Michael Nygard's original ADR post](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [`CONTRIBUTING.md`](../../CONTRIBUTING.md#adr-process)
