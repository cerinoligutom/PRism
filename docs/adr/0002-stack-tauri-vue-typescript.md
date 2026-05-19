# 0002 — App stack: Tauri 2 + Vue 3 + TypeScript

- **Status:** Accepted
- **Date:** 2026-05-19
- **Issue:** [#1](https://github.com/cerinoligutom/PRism/issues/1)
- **Deciders:** @cerinoligutom

## Context

PRism is a cross-platform desktop dashboard for GitHub PRs. Three classes of stack are viable: native (per-OS toolkits), Electron-style (Chromium + Node), and Tauri-style (system webview + a compiled native core). The non-functional requirements (PRD §8) call for a binary under 20 MB, cold start under 2 seconds, memory under 200 MB with 500 PRs cached, and native OS integrations (keychain, notifications, menu bar).

The frontend layer needs to be productive for a small team, well-supported by AI coding agents, and have an accessible headless component ecosystem.

## Decision drivers

- Binary size and startup time (PRD §8.1).
- Memory footprint and runtime overhead.
- Native OS integration depth (keychain, notifications, menu bar / dock badges).
- Developer velocity and ecosystem maturity.
- Cross-platform from one codebase (macOS, Windows, Linux).
- AI-agent tooling coverage for the chosen languages.

## Considered options

1. **Electron + Vue/TypeScript** — mature, large ecosystem, but ~80 MB binaries, ~150 MB RAM idle, slow cold start, Chromium-update treadmill.
2. **Native per-OS (SwiftUI + WinUI + GTK)** — best UX and footprint, ~3x development cost.
3. **Tauri 2 + Vue 3 + TypeScript** — Rust core + system webview, ~15 MB binaries, native bindings, single codebase.
4. **Web app** — eliminated by the requirement for OS keychain access.

## Decision

We will build PRism on **Tauri 2** with a **Vue 3 + TypeScript** frontend.

- Build tool: Vite.
- UI components: [Reka UI](https://reka-ui.com/) (headless, accessible, Vue port of Radix).
- Styling: Tailwind CSS.
- State: Pinia.
- Routing: Vue Router (when multi-view navigation is needed).
- Utilities: VueUse for composables (storage, sensors, async, time).
- Backend (in-process): Rust, leveraging Tauri APIs for keychain, notifications, and tray.

Rationale: Tauri hits the binary-size and memory targets in PRD §8, exposes the native integrations PRism needs (keychain, notification centre, dock badge), and lets us write the sync/storage/parsing layer in Rust where strict types and concurrency primitives pay off. Vue + TS keeps the UI work approachable without the React-ecosystem churn.

## Consequences

### Positive

- Meets the size, memory, and startup targets without aggressive optimisation.
- Native OS integrations are first-class, not bolted on.
- Rust backend gives us a place to put performance-sensitive logic (timeline reconstruction, conversation stats) with predictable performance.
- Headless components (Reka UI) keep us free of CSS-in-JS frameworks and let us own the design tokens.

### Negative

- Tauri's ecosystem is smaller than Electron's; some niche libraries don't exist or are pre-1.0.
- Webview differences across OSes (WebKit on macOS, WebView2 on Windows, WebKitGTK on Linux) create test surface.
- Two-language stack raises the bar for contributors slightly.

### Neutral / follow-ups

- Linter / formatter wiring (rustfmt, clippy, ESLint, Prettier) is a future PR after the scaffold lands.
- A future ADR will lock the Tauri version policy (latest stable vs LTS).

## References

- [Tauri 2](https://tauri.app/)
- [Reka UI](https://reka-ui.com/)
- [VueUse](https://vueuse.org/)
- PRD §7.1, §8.1
