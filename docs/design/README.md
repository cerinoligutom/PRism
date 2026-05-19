# Design reference

This directory is the read-only design reference for PRism's UI. Source artboards (HTML/CSS prototypes) live here and the live app imports adapted copies under `src/assets/styles/`.

The artboards were mocked up in [Claude Design](https://claude.ai/design) from the PRD alone, before this repo's ADRs and CONTRIBUTING existed. Treat them as the source of truth for **visual intent** but cross-check against M1's acceptance criteria — the dashboard, expanded PR row, and filter UI are M2+ work, not M1.

## Files

| Path | Purpose |
|---|---|
| `tokens.css` | Design tokens — OKLCH colour scale, type scale, spacing, radii, density vars |
| `app.css` | Shared UI primitive styles (btn, badge, avatar, nav-item, chip, kbd, scroll, etc.) |
| `logo.svg` | Brand mark — refraction triangle with semantic-colour rays |
| `artboards/branding.html` | Logo, lockups, spectrum |
| `artboards/design-system.html` | Type, colour, components, density variants, status icons |
| `artboards/dashboard.html` | Dark dashboard with PR list — drives M2 list layout |
| `artboards/dashboard-light.html` | Light theme variant |
| `artboards/dashboard-expanded.html` | Per-thread preview + conversation stats + timeline (M3+) |
| `artboards/onboarding.html` | 3-step welcome → PAT entry → org/repo select (drives #10) |
| `artboards/settings.html` | Accounts, sync, appearance, notifications (Accounts panel drives #10) |
| `artboards/states.html` | Empty / sync error / rate limit / offline / expired PAT |
| `chat-2026-05-19-design-system.md` | Original Claude Design transcript — design intent and choices |

## Departures from the prototype in the real app

- **Native OS window chrome.** The artboards render a fake macOS title bar with traffic lights (`.mac-window`, `.mac-titlebar`, `.mac-light`). The Tauri app uses native chrome on each platform — those classes are intentionally absent from `src/assets/styles/primitives.css`.
- **Fonts.** The artboards `@import` Geist + JetBrains Mono from Google Fonts. The app does the same in `src/assets/styles/tokens.css`. A future PR may self-host the font files so the app works offline.
- **Density.** The artboards demonstrate three densities in the design-system frame. The app stores the selected density in Pinia and writes it to `:root[data-density]`; rows can read `var(--row-h)` to pick up the matching value.
- **Accent picker.** The artboards show six presets + custom. The app exposes the same presets via the theme store; the picker UI ships with the Appearance settings panel (M3).
