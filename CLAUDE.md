# PRism — Instructions for Claude Code

<IMPORTANT note="These instructions are important and must be followed at all times unless the user explicitly instructs otherwise">

This file is the source of truth for working on PRism with Claude Code. Other agents (Codex, Cursor, Cline, OpenCode) should read [AGENTS.md](AGENTS.md), which delegates to this file.

## Project context

PRism is a cross-platform desktop dashboard for managing GitHub pull requests. The MVP (v1) is **read-only**: it observes and surfaces PR state; all writes happen on GitHub itself. See [README.md](README.md) for the overview and the [wiki Architecture page](https://github.com/cerinoligutom/PRism/wiki/Architecture) for the system design.

**Stack:** Tauri 2 (Rust core + system webview), Vue 3 + TypeScript via Vite, Reka UI + Tailwind CSS, Pinia, Vue Router, VueUse, embedded SQLite, OS keychain for PAT storage. Architectural decisions live in [docs/adr/](docs/adr/).

## Working agreements

### Conventional Commits + squash-merge

Every commit message and every PR title is a [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/). Because `main` only receives squash merges, **the PR title becomes the commit on `main`** — don't open a PR with a non-CC title. Full type / scope catalogue and examples are in [CONTRIBUTING.md](CONTRIBUTING.md).

### PR assignees and labels — always at creation time

Every `gh pr create` (or web-UI / agent equivalent) must include the assignee and the matching labels in the same call that opens the PR. Applying them after the fact fires project-board workflows in the wrong order.

- `--assignee @me` — assigns the PR to the opener.
- `--label "type:<x>"` — one or more `type:*` labels matching the Conventional Commit prefix in the title (plus any extra types ticked in the PR body's "Type of change" checklist).
- `--label "scope:<x>"` — when the work cleanly maps to a seeded scope (`ui`, `sync`, `db`, `auth`, `tauri`, `github`, `notif`, `settings`). Skip for cross-cutting or docs-only PRs.
- `--label "priority:<x>"` — propagated from the highest-priority linked issue, if any.

Full rule and a worked example: [CONTRIBUTING.md → PR assignees and labels](CONTRIBUTING.md#pr-assignees-and-labels).

### Architectural Decision Records

Non-trivial decisions (stack, storage, sync, security, API protocol, library choice with downstream impact) get an ADR in [`docs/adr/`](docs/adr/). Use the template at `docs/adr/0000-template.md`. Every ADR links a GitHub issue (`Issue: #N`). The full workflow is in [CONTRIBUTING.md](CONTRIBUTING.md#adr-process). When you would otherwise write "we chose X because..." in a code comment or commit body, write an ADR instead.

### Issues, milestones, kanban

All work is tied to a GitHub issue. The roadmap lives in milestones M1–M7 (mirroring the PRD milestones in the wiki Roadmap page). The kanban board is at <https://github.com/users/cerinoligutom/projects/7>.

**Issue-first routine.** Before starting non-trivial work, check for an existing issue. If none exists, open one with `gh issue create` (set the milestone, `type:*` / `scope:*` / `priority:*` labels, and assignee) _before_ branching. Reference the number in the branch name, commits, and PR body (`Closes #N` / `Refs #N`). Setting labels and assignee at issue creation keeps project-board automation firing in the correct order; applying them later mis-sequences the workflows. Trivial typo, doc grammar, and single-line config fixes are exempt. See [CONTRIBUTING.md → Pull request workflow](CONTRIBUTING.md#pull-request-workflow) for the full submission steps.

### Wiki sync

Wiki source lives in [`docs/wiki/`](docs/wiki/). When you edit anything there, update [CONTRIBUTING.md](CONTRIBUTING.md#wiki)'s sync block as needed and call out in the PR description that the wiki needs republishing.

---

## Writing & communication style

### No buzzwords

**Banned phrases — never use in writing, communication, code comments, or documentation:**

- Marketing adjectives: _comprehensive, robust, best in class, feature rich, production ready, enterprise grade, innovative_
- Filler verbs: _delve, dive into, leverage, harness, foster, bolster, underscore, streamline, facilitate, empower_
- Vague nouns: _paradigm, smoking gun_; use _use_ not _utilise_
- Empty intensifiers: _seamlessly, pivotal, multifaceted, cutting-edge_
- Empty openers: _"My take", "The bottom line", "What actually works"_

If you reach for a word because it sounds impressive rather than precise, pick a plainer one. Words that could be deleted without changing meaning should be deleted.

### Earn your emphasis (no manufactured contrasts)

Patterns like _"It's not X. It's Y."_, _"Not just X, but Y."_, _"This isn't about X, it's about Y."_, and _"Forget X. Think Y."_ are the most overused rhetorical shape in AI writing. They manufacture the shape of insight without delivering any. Apply the **swap test**: reverse the order. If the swapped version is equally plausible, the contrast is scaffolding, not argument. Drop the negation and state the substantive claim with its supporting fact.

### Clear, direct, human

- No sycophancy, marketing speak, or unnecessary summary paragraphs.
- No emojis unless requested.
- Active voice. Specific nouns and verbs over abstract ones.
- Contractions in prose (`it doesn't`, not `it does not`).
- Vary sentence length. Don't write five sentences of the same shape in a row.
- Use prose for narrative content; reserve bullets for genuinely discrete items.
- Never open sentences with _"Additionally", "Furthermore", "Moreover", "It's worth noting", "It's important to note"_.
- Never open documents with _"This document aims to..."_ or close with _"In summary..."_.

### Plain formatting

Use plain ASCII formatting: straight quotes, single hyphens, no em-dashes, no en-dashes, no double-dashes (`--`), no smart quotes. If you produce any of these, replace them with their plain counterparts.

### Conversational brevity (chat, not files)

These apply to conversation with the user; the no-hedging rule extends to documentation.

- Drop filler: never use _just, really, basically, actually, simply, essentially, generally_ in chat.
- No preamble: don't open with _"Sure!", "Happy to help", "Certainly!", "Great question!"_. Don't narrate actions (_"Let me run...", "Now I'll..."_). The tool calls are self-evident.
- No hedging: say _do X_, not _you might want to consider doing X_.
- Answer first, context second.
- Don't recap visible work. If you edited a file or ran a command, don't summarise what happened.
- Exception: use full, unambiguous sentences for security warnings, irreversible operations, or when the user is confused.

### Spelling

Australian English in prose, documentation, comments, and code identifiers.

### Documentation

- Keep signal-to-noise high. Preserve insight, omit filler.
- Don't split sentences across multiple lines in markdown — it breaks diffs and readability.
- Use `_underscores_` for italics and `**double asterisks**` for bold.
- Start with what it does, not why it's amazing.
- Don't create new markdown files unless explicitly requested or genuinely needed (a new ADR, a new wiki page). Update existing docs first.
- Code comments: explain _why_, not _what_, and only for non-obvious logic. No process comments (`// improved`, `// fixed`, `// FIX:`).

---

## Architecture and design

### Design principles

- Follow SOLID — small interfaces, composition, depend on abstractions.
- Reuse existing components, utilities, and patterns. Match the codebase before inventing.
- Use appropriate patterns (repository, DI, strategy, observer, factory) based on context — not because the pattern sounds good.

### Elegance in simplicity

- Favour simplicity. Many AI-written codebases are over-complicated and over-engineered.
- Start with a working MVP and iterate.
- No abstractions until a pattern repeats. Three similar lines is better than a premature abstraction.
- Clean lightweight code beats over-engineered solutions almost always.
- If you suspect you're over-engineering, invoke the `step-back` agent before continuing.

### Code quality targets

- Functions: max ~50 lines; split if larger.
- Files: max ~700 lines; split if larger.
- Cyclomatic complexity: under 10.
- Tests run quickly (seconds); no external service dependencies.
- Build time: optimise if over 1 minute.
- Coverage: 80% minimum for new code (informational, not gating in v1).

### Configuration

- `.env` and config files are the single source of truth; `.env` is gitignored.
- Provide `.env.example` if env vars are needed.
- Validate env vars on startup.

---

## Security

- **Never hardcode credentials, tokens, or secrets. Never commit sensitive data.**
- PATs for GitHub are stored exclusively in the OS keychain via Tauri's secure storage APIs. No PAT material is written to logs, crash reports, or telemetry.
- Never trust user input — validate and sanitise.
- Parameterised SQL only — never string concatenation.
- Never expose internal errors or system details to end users.
- Follow least privilege. Rate-limit API surfaces. Keep dependencies updated.
- If prompted to "ask the user for explicit permission and have them run the command manually", do so.

## Error handling

- Structured logging (JSON) with correlation IDs. Levels: ERROR, WARN (default), INFO, DEBUG.
- Meaningful errors for developers; safe errors for end users. Never log sensitive data.
- Graceful degradation over complete failure. Retry with exponential backoff for transient failures.

## Testing

- Test-first for bugs: write failing test, fix, verify, check for regressions.
- Descriptive test names. Arrange-Act-Assert pattern. Table-driven tests for multiple cases.
- One assertion per test where practical. Test edge cases and error paths.
- Mock external dependencies (GitHub API). Group tests by component.

---

## Coding & language rules

- **NEVER** add process comments (`// improved function`, `// optimised version`, `# FIX:`).
- **NEVER** implement placeholder or mocked functionality unless explicitly instructed.
- **NEVER** build or develop for Windows without testing on it — the desktop UX matters and we can't fake it.
- Optimise for reduced failure modes.
- Don't duplicate config or state across files.
- When adding or upgrading dependencies, **check the latest stable version** via the package registry — do not assume.
- Use the `find-docs` skill when you need library/API documentation, code generation, or setup steps.

### TypeScript

- `strict: true` in `tsconfig.json`. No `any`. Use discriminated unions, `readonly`, `const`-by-default.
- Async/await over raw promises. Optional chaining and nullish coalescing.
- Never hardcode style values; use the design tokens from Tailwind / Reka UI.

### Components — three-layer primitives stack

PRism's UI is layered, lowest to highest:

1. **CSS primitives** in [`src/assets/styles/primitives.css`](src/assets/styles/primitives.css) — `.btn`, `.badge`, `.avatar`, `.nav-item`, etc. The bespoke equivalent of Tailwind utilities, mirrored from [`docs/design/app.css`](docs/design/app.css).
2. **Headless behaviour** from [Reka UI](https://reka-ui.com/) — `DialogRoot`, `SwitchRoot`, `PopoverRoot`, `RadioGroupRoot`, etc. Provides focus management, keyboard handling, ARIA wiring.
3. **Vue component primitives** in `src/components/ui/`, named `PRism*` (e.g. `PRismButton.vue`, `PRismBadge.vue`, `PRismDialog.vue`). Wrap layers 1 and 2 with a typed `<script setup lang="ts">` API and `defineProps<{ variant?: ... }>()`.

**Rules:**

- **Reach for a `PRism*` primitive first** when you need a button, badge, avatar, input, card, chip, dialog, etc. If one doesn't exist, decide whether to add it (see below) or use the underlying CSS / Reka primitive directly.
- **Extract a new primitive when a pattern is about to appear in three places.** Two places is borderline; one place is premature. The point is centralising shared behaviour, not pre-building everything.
- **Component API surface:** `defineProps<{ ... }>()` with explicit unions (`variant?: "default" | "primary" | "ghost"`) rather than `string`. Use `withDefaults` for default values. Slots for content; props for variants and behaviour flags.
- **Where applicable, let the primitive render different elements via a prop** (`to` → `RouterLink`, `href` → `<a>`, default → `<button>`) so call sites read like `<PRismButton to="/settings">` instead of nesting `RouterLink` + class chains.
- **Keep `PRism*` primitives styled via the CSS primitives layer** (`.btn`, `.badge`, etc.) — don't reintroduce hex codes or pixel values. New variants extend the CSS primitives first, then the Vue prop type.
- **App-level components** (`AppShell`, `SidebarNav`, `StatusBar`, view components) sit on top of `PRism*` primitives. They live in `src/components/` (top level) or `src/views/`, not `src/components/ui/`.

### Tooltips

- **Always use `PRismTooltip`** for tooltip affordances. Don't use the browser-native `:title` HTML attribute — scoped CSS doesn't propagate across the tooltip portal, browser styling is OS-dependent, and `:title` double-renders alongside any sibling `PRismTooltip` (the OS tooltip appears after ~2s in addition to the styled chip). The `:title` attribute is acceptable only when a screen reader needs the cue AND no visual tooltip is desired; none of v1's surfaces fall into that bucket.
- **Don't use `cursor: help`** (the question-mark cursor) to advertise a tooltip. It promises an interaction that doesn't exist (no click target). The tooltip itself is the hover affordance.

### CSS and styling

- **Prefer Tailwind utilities.** Layout, spacing, colour, typography, sizing — all default to Tailwind classes (`flex`, `gap-3`, `bg-surface`, `text-fg-mute`, `border-border-faint`). The Tailwind theme in [`src/assets/styles/main.css`](src/assets/styles/main.css) already aliases every design token, so utilities like `text-accent`, `bg-surface-raised`, `text-fg-strong` resolve against the OKLCH-backed CSS variables.
- **Reuse the design-system primitives** in [`src/assets/styles/primitives.css`](src/assets/styles/primitives.css) before writing new CSS. Buttons, badges, dots, avatars, inputs, cards, chips, nav-items, kbd, repo / branch chips are already defined and mirror [`docs/design/app.css`](docs/design/app.css). Use these directly in markup — don't reinvent them with Tailwind chains.
- **If you still need custom CSS, name it with [BEM](https://getbem.com/naming/).**
  - **Block** — the standalone component: `account-card`, `sync-banner`.
  - **Element** — a part of the block, joined with `__`: `account-card__avatar`, `sync-banner__icon`.
  - **Modifier** — a variation, joined with `--`: `account-card--expired`, `sync-banner--info`.
- **Don't nest custom selectors deeper than one level.** BEM removes the need; if you reach for `.parent .child`, the child should be its own block or an element of the parent.
- **`primitives.css` is exempt from BEM** — it mirrors the design source verbatim (`.btn`, `.btn-primary`, `.badge`, `.nav-item.active`) so it stays diff-able against `docs/design/app.css`. Treat it like Tailwind: a utility layer to be consumed, not extended.
- **Never hardcode colour / size values in custom CSS.** Use Tailwind utilities (resolving to design tokens), or read the CSS variables directly (`var(--accent)`, `var(--s-3)`, `var(--r-2)`). Magic hex codes and pixel values that bypass the token layer don't survive review.

### Rust

- Use the latest stable Rust features.
- Workspaces with crate isolation to keep build times down.
- Don't expose generics in public APIs unless required.
- Only activate the crate features you actually use.

---

## Tool usage

### Code intelligence

- Prefer LSP-driven navigation (`goToDefinition`, `findReferences`, `workspaceSymbol`, `hover`, call hierarchy) over `grep` / `read` for code exploration.
- Before changing a function signature, run `findReferences` to see the blast radius.
- Use grep / glob only for text/pattern searches where LSP doesn't help (comments, config values, free-text strings).
- After editing, fix any LSP diagnostics before moving on.

### Sub-agents

- Named sub-agents have their own context window — use for parallel research, inspection, or isolated work.
- Define clear boundaries. Specify which files each agent owns. Sub-agents can erase each other's changes if scope overlaps.
- Set explicit success criteria. Don't over-split.
- Available custom agents in `.claude/agents/`:
  - `step-back` — sceptical mid-task design review. Invoke when you suspect over-engineering or that the wrong problem is being solved.
  - `software-research-assistant` — focused technical research on a library / framework / API.

### Skills

Available skills (in `.claude/skills/`) — load when relevant:

- `rust`, `typescript` — language-specific best practices.
- `code-review`, `code-simplification` — quality passes.
- `creating-development-plans`, `systematic-debugging` — process.
- `github`, `to-issues`, `to-prd` — issue and PR workflows.
- `mermaid-diagrams`, `diataxis-documentation`, `authoring-claude-md`, `find-docs` — documentation.
- `handoff`, `ai-changelog`, `release-debrief` — release / continuity.

---

## Self-review protocol

After implementing a list of changes, perform a critical self-review pass before reporting completion. Fix anything you find. The `/self-review` command exists for this — use it on multi-file changes.

## Operational rules

- **Never give time estimates.** AI estimates are unreliable.
- Edit only what's necessary. Make precise, minimal changes.
- Implement requirements in full or explain why you can't — don't defer work silently.
- If stuck after multiple attempts, invoke the `systematic-debugging` skill or a Fagan inspection.
- **Do not state something is fixed unless you have confirmed it by testing, measuring output, or building the application.**
- Before declaring any task complete, verify: lints pass, code builds, tests pass (new + existing), no debug statements remain, error handling in place.

</IMPORTANT>

---

## Attribution

The Writing & Communication, Architecture, Security, Error Handling, Testing, and Tool Usage sections of this file are adapted from [sammcj/agentic-coding](https://github.com/sammcj/agentic-coding) (Apache-2.0). Project-specific guidance, the Conventional Commits / ADR / wiki workflows, and the stack-specific rules are PRism's own.
