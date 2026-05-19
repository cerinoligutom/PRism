<!--
PR title must be a Conventional Commit, e.g.:
  feat(github): GraphQL query for review thread resolution
  fix(sync): handle 304 with no body on conditional GET
  docs(adr): record decision to use SQLite for local cache

It becomes the squash commit message on `main`.
-->

## Summary

<!-- 1-3 sentences. What changed and why. -->

## Linked issues

Closes #
<!-- Use additional `Refs #N` lines for related issues. -->

## Type of change

<!-- Tick all that apply. Must match the prefix in the PR title. -->

- [ ] `feat` — new user-visible functionality
- [ ] `fix` — bug fix
- [ ] `docs` — documentation only
- [ ] `style` — whitespace / formatting only
- [ ] `refactor` — no behaviour change
- [ ] `perf` — performance improvement
- [ ] `test` — tests only
- [ ] `build` — build system, packaging, dependencies
- [ ] `ci` — CI configuration
- [ ] `chore` — repo plumbing / housekeeping
- [ ] `revert` — reverts a prior commit (reference SHA in body)
- [ ] Breaking change (add `!` to type and a `BREAKING CHANGE:` footer)

## ADR

<!-- Did this change make a non-trivial decision? If yes, add the ADR in this PR and link it. -->

- [ ] No ADR needed for this change.
- [ ] ADR added / updated: `docs/adr/NNNN-...md`

## Test plan

<!--
Required. Steps the reviewer can run to verify. Don't write "tests pass" — describe
WHAT you verified and HOW. Be specific about the golden path and any edge cases.
-->

## Screenshots / recordings

<!-- For UI changes. Delete this section if N/A. -->

## Checklist

- [ ] PR title is a Conventional Commit and matches the type ticked above
- [ ] At least one linked issue (`Closes #N` or `Refs #N`)
- [ ] Code builds and type-checks locally
- [ ] Lints / formatters pass (Rust + TS)
- [ ] New tests added where appropriate; all tests pass
- [ ] No secrets, no debug statements, no placeholder code
- [ ] Documentation updated (README / CONTRIBUTING / wiki source / code comments)
- [ ] Wiki source in `docs/wiki/` updated and the PR description notes if a mirror push is needed
- [ ] ADRs added / updated where a non-trivial decision was made
- [ ] Self-review pass completed before requesting review
