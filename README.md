# PRism

Cross-platform desktop dashboard for managing GitHub pull requests.

PRism gives developers, reviewers, and engineering leads a single focused dashboard for every PR they care about — across repos, orgs, and GitHub accounts — with deeper visibility into conversation state, status timelines, and review signals than GitHub's native UI provides. V1 is **read-only**: PRism observes and surfaces state; review, comment, and merge actions happen on GitHub itself.

## Status

Pre-alpha. M1 (foundations) is in progress — application shell is in place; sync, storage, and auth are open issues on the [kanban board](https://github.com/users/cerinoligutom/projects/7).

## Stack

- [Tauri 2](https://tauri.app/) (Rust core + system webview)
- Vue 3 + TypeScript via Vite
- [Reka UI](https://reka-ui.com/) headless components + Tailwind CSS
- Pinia, Vue Router, [VueUse](https://vueuse.org/)
- SQLite (embedded) for local cache
- OS keychain for PAT storage (macOS Keychain / Windows Credential Manager / libsecret)

See [docs/adr/0002-stack-tauri-vue-typescript.md](docs/adr/0002-stack-tauri-vue-typescript.md) for the decision record.

## Getting started

Prerequisites and dev setup live in the wiki: [Getting Started](https://github.com/cerinoligutom/PRism/wiki/Getting-Started). The short version:

```bash
pnpm install
pnpm tauri:dev      # native window, hot reload
pnpm tauri:build    # release binary under src-tauri/target/release/bundle/
```

You need Node 22+, pnpm 11+, and Rust stable. Tauri's [Prerequisites guide](https://tauri.app/start/prerequisites/) covers the OS-specific bits.

## Project layout

```
.
├── .claude/            # Agentic-coding rules, agents, commands, skills (for Claude Code)
├── .github/            # Issue + PR templates, CODEOWNERS
├── docs/
│   ├── adr/            # Architectural Decision Records
│   ├── contracts/      # Cross-module interface specs (e.g. github-client)
│   ├── design/         # Source design tokens, prototype artboards, brand
│   └── wiki/           # Source for the GitHub wiki (mirrored)
├── src/                # Vue 3 frontend (Tauri webview)
│   ├── assets/styles/  # tokens.css, primitives.css, Tailwind entry
│   ├── components/     # AppShell, SidebarNav, StatusBar
│   ├── views/          # Dashboard, Settings, Onboarding
│   ├── stores/         # Pinia stores
│   └── router/         # Vue Router
├── src-tauri/          # Rust backend + Tauri config
├── AGENTS.md           # Instructions for non-Claude agents
├── CLAUDE.md           # Instructions for Claude Code
├── CONTRIBUTING.md     # Commit style, PR flow, ADR process
├── LICENSE
└── README.md
```

## Contributing

- Issues and roadmap: [GitHub kanban board](https://github.com/users/cerinoligutom/projects/7)
- Conventions: [CONTRIBUTING.md](CONTRIBUTING.md)
- Architectural decisions: [docs/adr/](docs/adr/)
- Long-form docs and runbooks: [wiki](https://github.com/cerinoligutom/PRism/wiki)

All work follows [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/); see [CONTRIBUTING.md](CONTRIBUTING.md) for the type/scope conventions used here.

## Licence

[MIT](LICENSE).
