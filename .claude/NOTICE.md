# NOTICE

The agentic-coding rules and assets under `.claude/` and portions of `CLAUDE.md` are adapted from [sammcj/agentic-coding](https://github.com/sammcj/agentic-coding), licensed under Apache-2.0.

## Files derived from `sammcj/agentic-coding`

- `.claude/agents/step-back.md`
- `.claude/agents/software-research-assistant.md`
- `.claude/commands/self-review.md`
- `.claude/commands/compact-prep.md`
- `.claude/skills/` — curated subset:
  - `ai-changelog/`, `authoring-claude-md/`, `code-review/`, `code-simplification/`, `creating-development-plans/`, `diataxis-documentation/`, `find-docs/`, `github/`, `handoff/`, `mermaid-diagrams/`, `release-debrief/`, `rust/`, `systematic-debugging/`, `to-issues/`, `to-prd/`, `typescript/`
- Portions of `CLAUDE.md` (Writing & Communication, Architecture, Security, Error Handling, Testing, Tool Usage sections).

## Modifications

- User-specific paths and tooling references (`/Users/samm/…`, `run_silent`, custom statusline) removed.
- Project-specific guidance, Conventional Commits / ADR / wiki workflows, and stack-specific (Tauri / Vue / TS / Rust) rules added.
- `.claude/settings.json` rewritten from scratch, retaining the permission-list shape and the safety denylist patterns.

## Upstream licence

Apache License 2.0 — see <https://github.com/sammcj/agentic-coding/blob/main/LICENSE>.
