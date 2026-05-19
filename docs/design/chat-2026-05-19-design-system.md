# Design System & Theming

_Started 2026-05-19 12:50 UTC_

---

## User

I'd like you to create a design system and designs for this PRD. Let me know if you have questions. Would like to support dark, light and possibly custom theming.

## Assistant

_[tool: read_file]_

_[tool: questions_v2]_

I've read the PRD — PRism is a clear, focused product: a local-first Tauri desktop app for unified GitHub PR management with deep conversation visibility. Before I start designing, a few questions will dramatically improve the output.

Quick note: I'll avoid copying GitHub's UI directly and instead build PRism its own visual identity that *plays well with* GitHub data (similar PR semantics, original look).

## User

Questions answered:
- scope: Design system page (tokens, type, color, components), Main dashboard screen (the PR list — the hero view), Expanded PR row / per-thread preview state, Settings + account management, Onboarding (PAT entry, scopes explainer), Empty / error / stale-sync states, Decide for me
- vibe: Quiet, neutral, dense — like Linear: monochrome, sharp, dev-tool feel
- density: Toggleable density (tight/comfortable/roomy)
- accent: 5
- themes: Dark + light + accent picker only (simplest custom)
- default_theme: Dark — most devs default to dark
- typography: Geometric sans (Inter-alt like Geist) + JetBrains Mono for numbers/code
- variations: Several full-screen options on a design canvas (compare side-by-side)
- row_layout_priority: Title + repo identity, Conversation state (unresolved threads, mentions), Reviewer state (who's approved / who's pending), Time since last status change
- novelty: Conservative — proven dev-tool patterns, low risk
- interactive_depth: Full prototype + onboarding/PAT flow + notification preferences
- platform_chrome: macOS window chrome (traffic lights, title bar)
- extras: Make sure the colors are WCAG AA compliant especially the text contrast.

Feel free to create a logo. Also create a branding page.

## User

Questions answered:
- scope: Design system page (tokens, type, color, components), Main dashboard screen (the PR list — the hero view), Expanded PR row / per-thread preview state, Settings + account management, Onboarding (PAT entry, scopes explainer), Empty / error / stale-sync states, Decide for me
- vibe: Quiet, neutral, dense — like Linear: monochrome, sharp, dev-tool feel
- density: Toggleable density (tight/comfortable/roomy)
- accent: 5
- themes: Dark + light + accent picker only (simplest custom)
- default_theme: Dark — most devs default to dark
- typography: Geometric sans (Inter-alt like Geist) + JetBrains Mono for numbers/code
- variations: Several full-screen options on a design canvas (compare side-by-side)
- row_layout_priority: Title + repo identity, Conversation state (unresolved threads, mentions), Reviewer state (who's approved / who's pending), Time since last status change
- novelty: Conservative — proven dev-tool patterns, low risk
- interactive_depth: Full prototype + onboarding/PAT flow + notification preferences
- platform_chrome: macOS window chrome (traffic lights, title bar)
- extras: Make sure the colors are WCAG AA compliant especially the text contrast.

Feel free to create a logo. Also create a branding page.

## Assistant

_[tool: update_todos]_

_[tool: copy_starter_component]_

Got it. Setting up PRism — let me plan the build, then go.

_[tool: read_file]_

_[tool: grep]_

_[tool: write_file]_

_[tool: write_file]_

Now I'll build the shared design tokens first, then each artboard.

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: write_file]_

_[tool: copy_files]_

_[tool: str_replace_edit]_

_[tool: write_file]_

_[tool: update_todos]_

_[tool: register_assets]_

_[tool: done]_

_[tool: fork_verifier_agent]_

Done. PRism designs delivered as a side-by-side canvas.

**What's in the canvas:**

- **Brand & system** — logo with refraction mark, lockups, construction notes, spectrum + a 7-section design system (type scale, surface/text/status palettes with measured AA contrast, accent picker, spacing, radii, density variants, every UI component, status icon set, PR-row anatomy)
- **Dashboard** — dark default + light variant + expanded PR state with per-thread preview, conversation stats (oldest unresolved, avg response, resolution rate), status timeline, per-check CI, reviewer detail
- **Flows** — 3-step onboarding (welcome → fine-grained PAT entry → org/repo selection → first sync) and Settings (accounts with expired-PAT banner, sync sliders, theme + accent picker, per-event notification matrix)
- **States** — first-run empty, filter empty, sync error, rate-limit warning, archived, offline, plus a wide expired-PAT blocker

**Design choices to flag:**

- **Accent = magenta** (`oklch(0.72 0.14 320)`) — quiet but distinct from GitHub's blue/purple, named after refraction. Six other accents pre-curated in settings + custom picker.
- **Text contrast** — every text token measured against `--bg-1`: strong 17.5:1, body 12.4:1, mute 6.8:1. Faint (3.6:1) is reserved for ≥18px / icons only and labeled as such on the swatch card.
- **PR row reading order** matches your priority: title+repo → conversation state (segmented thread bar) → reviewer state (avatar dots + summary) → time-in-status. CI and kebab sit on the right.
- **3 densities** built in — comfortable is default; the same three are shown in the design system.
- **Conservative novelty** — the one slightly-novel element is the 3-color **thread segment bar** (unresolved / you're-in / resolved) which encodes the PRD's "conversation stats" priority in a single 90px glyph.

**Caveats / next steps:**

- Avatars and org logos are placeholder initials — drop real GitHub avatars when wired up.
- Light dashboard is a quick theme flip; could use a dedicated pass on hover/attention tinting if you want it equally polished.
- I didn't ship a notification toast spec or menu-bar mockup — happy to add if useful.

