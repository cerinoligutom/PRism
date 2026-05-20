# Getting Started

This page covers the prerequisites and the first build of PRism.

## Prerequisites

PRism is built on Tauri 2 (ADR [0002](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0002-stack-tauri-vue-typescript.md)). Follow Tauri's [Prerequisites guide](https://tauri.app/start/prerequisites/) for your OS. The short version:

### macOS

- Xcode Command Line Tools: `xcode-select --install`
- Rust (latest stable): <https://rustup.rs>
- Node 24+ LTS (use `nvm`, `fnm`, or your package manager)
- pnpm 11+: `corepack enable pnpm`

### Windows

- [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload
- [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (ships with Windows 11, install on 10)
- Rust (latest stable): <https://rustup.rs>
- Node 24+
- pnpm 11+

### Linux

- Standard build chain: `gcc`, `pkg-config`, `libssl-dev`
- WebKitGTK and dependencies — see [Tauri Linux prereqs](https://tauri.app/start/prerequisites/#linux)
- Rust (latest stable)
- Node 24+
- pnpm 11+

## Clone and run

```bash
git clone git@github.com:cerinoligutom/PRism.git
cd PRism
pnpm install
pnpm tauri:dev
```

`pnpm tauri:dev` builds the Rust core and opens a native window with the Vue UI hot-reloading from Vite. The first run takes a minute or two; subsequent runs are seconds.

For a release build:

```bash
pnpm tauri:build
```

Output binaries land under `src-tauri/target/release/bundle/`.

Frontend-only iteration (no native window, just the Vue UI in a browser at `http://localhost:5173`):

```bash
pnpm dev
```

## Authentication

PRism reads GitHub data with a Personal Access Token (PAT), stored exclusively in the OS keychain (ADR [0005](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0005-pat-auth-and-keychain-storage.md)). Fine-grained PATs are recommended.

On first launch, open Settings → Accounts → Add account. The app links to GitHub's PAT creation page with the scopes pre-filled.

Required scopes:

- **Classic PAT:** `repo` (read), `read:org`, `read:user`. Note: `repo` includes write — PRism never writes; consider fine-grained instead.
- **Fine-grained PAT:** Read access to **Contents**, **Issues**, **Pull requests**, **Metadata**, **Members**, **Profile**.

## Development workflow

- Conventions (commit style, PR flow, ADRs): see [Conventions](Conventions) and the source [CONTRIBUTING.md](https://github.com/cerinoligutom/PRism/blob/main/CONTRIBUTING.md).
- Issues live on the [kanban board](https://github.com/users/cerinoligutom/projects/7).
- For new contributions, pick an unassigned issue in the active milestone and follow the PR template.

## Troubleshooting

This section grows with the project. Open an issue with the bug-report template if you hit something not listed here.
