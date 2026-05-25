<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/design/logo-dark.svg">
  <img src="docs/design/logo-light.svg" alt="PRism logo" width="120" height="120">
</picture>

# PRism

**Every PR you touch, in one quiet place. See the state, not just the noise.**

A cross-platform desktop dashboard for managing GitHub pull requests.

[![CI](https://github.com/cerinoligutom/PRism/actions/workflows/ci.yml/badge.svg)](https://github.com/cerinoligutom/PRism/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/cerinoligutom/PRism?include_prereleases&sort=semver&label=release)](https://github.com/cerinoligutom/PRism/releases/latest)
[![Licence](https://img.shields.io/github/license/cerinoligutom/PRism?label=licence)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-555)](#install)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24C8DB)](https://tauri.app/)

[Install](#install) · [First launch](#first-launch) · [Development](#getting-started-development) · [Contributing](#contributing) · [Wiki](https://github.com/cerinoligutom/PRism/wiki)

</div>

---

## About

PRism gives developers, reviewers, and engineering leads a single focused dashboard for every PR they care about, across repos, orgs, and GitHub accounts, with deeper visibility into conversation state, status timelines, and review signals than GitHub's native UI provides. V1 is **read-only**: PRism observes and surfaces state; review, comment, and merge actions happen on GitHub itself.

## Status

Pre-alpha. M1 (foundations) is in progress: application shell is in place; sync, storage, and auth are open issues on the [kanban board](https://github.com/users/cerinoligutom/projects/7).

For v1, github.com is the only validated host; GitHub Enterprise hosts have the wiring in place but are not validated. See ADR [0016](docs/adr/0016-unified-multi-account-dashboard.md).

## Stack

- [Tauri 2](https://tauri.app/) (Rust core + system webview)
- Vue 3 + TypeScript via Vite
- [Reka UI](https://reka-ui.com/) headless components + Tailwind CSS
- Pinia, Vue Router, [VueUse](https://vueuse.org/)
- SQLite (embedded) for local cache
- OS keychain for PAT storage (macOS Keychain / Windows Credential Manager / libsecret)

See [docs/adr/0002-stack-tauri-vue-typescript.md](docs/adr/0002-stack-tauri-vue-typescript.md) for the decision record.

## Install

Grab the installer for your OS from the [Releases page](https://github.com/cerinoligutom/PRism/releases/latest). PRism v1 ships **unsigned at the OS level** (see [ADR-0023](docs/adr/0023-release-pipeline.md)): the binaries are signed with Tauri's updater key for OTA integrity, but no Apple Developer ID and no Windows code-signing cert. Expect a one-time per-release click-through on macOS and Windows. Linux launches without a warning.

### macOS (Intel and Apple Silicon)

1. Download the matching `.dmg` from the latest release:
   - **Apple Silicon:** `PRism_X.Y.Z_aarch64.dmg`
   - **Intel:** `PRism_X.Y.Z_x64.dmg`
2. Open the `.dmg` and drag **PRism** into **Applications**.
3. First launch: right-click `PRism.app` in Applications and pick **Open**, then confirm the Gatekeeper prompt. Macs block unsigned apps when you double-click them; the right-click route is the only way through on first run.

Prefer the command line:

```bash
xattr -d com.apple.quarantine /Applications/PRism.app
```

The Gatekeeper warning recurs on the first launch after every auto-update, because macOS re-applies the quarantine attribute to the replaced bundle. Run the right-click-Open dance (or the `xattr` command) once per release.

### Windows x64

1. Download `PRism_X.Y.Z_x64_en-US.msi` from the latest release.
2. Double-click the `.msi`. Windows SmartScreen will show **"Windows protected your PC"**.
3. Click **More info**, then **Run anyway** _(screenshot: SmartScreen warning)_. The installer then walks you through the standard MSI flow.

SmartScreen recurs on the first launch after every auto-update; same **More info -> Run anyway** click-through.

### Linux x64

Pick whichever format suits your distro. Both come from the latest release page.

**AppImage** (portable, works on any glibc-based distro):

```bash
chmod +x PRism_X.Y.Z_amd64.AppImage
./PRism_X.Y.Z_amd64.AppImage
```

**Debian / Ubuntu** (`.deb`):

```bash
sudo dpkg -i prism_X.Y.Z_amd64.deb
```

No OS-level warning on either path.

## First launch

The first time PRism opens you'll see a three-step onboarding flow.

### 1. Welcome

A short intro to what PRism does: a local, read-only dashboard that watches every PR you care about and surfaces conversation depth, reviewer status, and time-in-status across multiple GitHub accounts. Click **Connect GitHub** to move on.

### 2. Add a Personal Access Token

PRism authenticates against GitHub with a PAT, stored in the OS keychain. Two tabs are available:

- **Fine-grained** (recommended): narrower scope, explicit repository selection. PRism can't verify the granted permissions ahead of time (GitHub doesn't expose them), so tick the listed permissions on the PAT page before pasting the token.
- **Classic**: broader `repo` / `read:org` / `read:user` scopes. PRism reads the granted scopes back from GitHub and gates **Connect** until the required ones are present.

The form has an **Account label** (e.g. _Work_, _Personal_), a **Host** (`github.com` by default; use your GHES host for GitHub Enterprise, which is wired but not validated in v1), and a **Personal Access Token** field. Paste the token; PRism auto-detects whether it's fine-grained or classic from the prefix, switches the help to match, and validates the token on blur.

The **Create a new PAT** link on the form jumps to GitHub with the relevant scopes pre-filled. The minimum scopes are:

- **Fine-grained:** read-only on **Contents**, **Pull requests**, **Metadata**; plus **Members** (read-only) when the token's resource owner is an organisation.
- **Classic:** `repo`, `read:org`, `read:user`.

PRism never writes to GitHub. Approve / comment / merge actions all open GitHub itself.

See GitHub's docs for the full picture: [Managing your personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens).

### 3. First sync

Once the account is connected, PRism kicks off the first fetch in the background and confirms when it lands. From here you can **Add another account** (loops back to step 2) or **Open PRism** to land on the dashboard.

## Where to add more accounts later

Open **Settings -> Accounts -> + Add account** to walk through the same PAT flow at any time. The dashboard unifies PRs across every connected account by default (see [ADR-0016](docs/adr/0016-unified-multi-account-dashboard.md)).

To change which orgs and repos PRism watches, go to **Settings -> Repositories** and toggle entries on or off.

## Deep links

PRism registers the `prism://` custom URL scheme on install. Open any PR PRism is tracking by clicking a link of the form `prism://pr/<owner>/<repo>/<number>` (add `?host=ghes.example.com` for non-github.com hosts). Links to PRs PRism isn't tracking fall back to opening on GitHub. See the [wiki Architecture page](https://github.com/cerinoligutom/PRism/wiki/Architecture#deep-links) for the full URL shape.

## Getting started (development)

Prerequisites and dev setup live in the wiki: [Getting Started](https://github.com/cerinoligutom/PRism/wiki/Getting-Started). The short version:

```bash
pnpm install
pnpm tauri:dev      # native window, hot reload
pnpm tauri:build    # release binary under src-tauri/target/release/bundle/
```

You need Node 24+ LTS, pnpm 11+, and Rust stable. Tauri's [Prerequisites guide](https://tauri.app/start/prerequisites/) covers the OS-specific bits.

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
- Release gate: [Platform QA checklist](https://github.com/cerinoligutom/PRism/wiki/Platform-QA)

All work follows [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/); see [CONTRIBUTING.md](CONTRIBUTING.md) for the type/scope conventions used here.

## Licence

[MIT](LICENSE).
