# 0023 - Release pipeline: two-workflow split, draft GitHub Release, unsigned at OS level

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#297](https://github.com/cerinoligutom/PRism/issues/297)
- **Deciders:** @cerinoligutom

## Context

PRism is a Tauri 2 desktop app shipping to macOS (Intel + Apple Silicon), Windows x64, and Linux x64. v1.0 is days away from tag-cutting. The release path has been ad-hoc up to now (local `tauri build`, manual artefact uploads), which doesn't scale once the updater (ADR-0024) starts pulling a manifest off published releases on every app start.

Three concerns have to be settled before a v1.0 tag goes near the repo:

- **Trigger and review shape.** Anything that publishes a binary the public can download has to pass through human review. A direct push-to-tag-then-publish pipeline removes the review surface; tag-on-main with a draft release adds two.
- **OS-level signing.** macOS Gatekeeper blocks unsigned apps behind an explicit user override (right-click Open, or `xattr -d com.apple.quarantine`). Windows SmartScreen warns on unsigned binaries but is click-through. Linux has no OS-level signing requirement. We accept the user-override friction; Tauri's own updater signature is what gates binary trust on the OTA path.
- **Updater integration.** The updater plugin needs a signing keypair and a manifest URL. ADR-0024 owns the plugin and the manifest hosting; ADR-0023 owns the pipeline that produces signed artefacts and the URL pattern the manifest will follow. The updater keypair is independent of any OS code-signing cert.

## Decision drivers

- v1.0 is a launch, not a hotfix. Maintainer wants explicit review gates at three points: bumped versions, built binaries, and the final publish click.
- PRism is a non-profit personal project. Apple Developer Program membership ($99/yr) and Windows code-signing certs (commercial EV at hundreds per year, or SignPath OSS application friction) are out of scope. Users who want to run the app download from GitHub Releases and accept the OS-level click-through.
- Linux AppImage has no OS-level signing requirement. Tauri's own updater signature applies the same way it does on macOS / Windows.
- The Tauri updater plugin (ADR-0024) needs a free keypair from `tauri signer generate`. That keypair signs every installer and is what the updater verifies on OTA download. Independent of OS code signing, so OTA works whether or not the binaries carry an Apple Developer ID or Windows cert.
- Two failure modes to guard against: a bad version bump going public (catch at PR review), and a broken binary going public (catch in the draft release).

## Considered options

1. **Two-workflow split with draft Release + OS-unsigned + Tauri updater key (chosen).** `prepare-release.yml` opens a PR with version bumps and composed changelog; merging that PR tags `vX.Y.Z`; `release.yml` builds the matrix and attaches binaries to a *draft* GitHub Release; the maintainer publishes manually. All artefacts are signed by the Tauri updater key (free) so OTA works; no OS code-signing certs are provisioned.
2. **Single workflow on tag push.** One workflow that, on `vX.Y.Z` tag, builds and immediately publishes a non-draft Release. Loses the artefact-review gate; if the build is broken or signing failed silently the public sees it.
3. **Pay for OS-level signing.** Apple Developer ID ($99/yr) + Windows EV cert ($200-600/yr) or SignPath OSS approval flow. Removes the Gatekeeper / SmartScreen warnings. Rejected as disproportionate for a non-profit personal project; users absorb the install click-through instead.

## Decision

We will go with **Option 1**.

### Workflow split

**`prepare-release.yml` (workflow_dispatch only).** Triggered manually by the maintainer with a version input. The workflow:

- Bumps `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` to the requested version.
- Composes the `[Unreleased]` section of `CHANGELOG.md` into a `vX.Y.Z` heading.
- Opens a "release: vX.Y.Z" pull request against `main` with the diff.

The maintainer reviews the diff (correct version everywhere, changelog reads right) and squash-merges. Merging to `main` runs a post-merge step that pushes the `vX.Y.Z` tag.

**`release.yml` (triggered on tag push matching `v*`).** The tag push triggers a cross-platform matrix build:

- `macos-13` for Intel (`.dmg` + `.app`).
- `macos-14` (Apple Silicon runner) for ARM64 (`.dmg` + `.app`).
- `windows-latest` for Windows x64 (`.msi`).
- `ubuntu-22.04` for Linux x64 (`.AppImage` + `.deb`).

Each matrix leg signs its artefacts with the Tauri updater key and uploads them to a single **draft** GitHub Release named `vX.Y.Z`. The maintainer reviews the draft (artefacts present, names correct, edits release notes if needed) and clicks **Publish** when satisfied. Publishing emits the `release: published` event that ADR-0024's pipeline will react to.

### OS coverage

- macOS Intel + Apple Silicon: `.dmg` (drag-to-install) and `.app` (raw bundle for the updater payload). Ad-hoc signed by Tauri's bundler (no Developer ID); modern macOS refuses to launch fully unsigned binaries on Apple Silicon, so ad-hoc is the minimum.
- Windows x64: `.msi` only. No `.exe` from the NSIS bundler in v1.0.
- Linux x64: `.AppImage` (portable) and `.deb` (Debian/Ubuntu). No `.rpm` in v1.0.

ARM64 Windows and Linux are out of scope for v1.0.

### OS-level code signing: none

- **macOS:** ad-hoc signed only (Tauri's bundler default when no `signingIdentity` is configured). Keeps the binary launchable on modern macOS, but Gatekeeper still warns on first launch and on the first launch after every OTA update. Users right-click > Open the new bundle once per release, or run `xattr -d com.apple.quarantine /Applications/PRism.app`.
- **Windows:** unsigned. README install docs (#306) document the SmartScreen "More info > Run anyway" click-through. Same recurrence per release as macOS.
- **Linux AppImage:** no OS-level signing requirement; launches without warning.

No Apple Developer Program membership, no commercial Windows cert, no SignPath OSS application. Users tolerate one click-through per release in exchange for the project staying free to ship.

### Tauri updater keypair

Generated once via `tauri signer generate` and stored as `TAURI_SIGNING_PRIVATE_KEY` (the encoded private key) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the passphrase) in repository secrets. This pair signs **every installer on every platform** so the updater can verify downloads regardless of OS. The public counterpart is committed to `src-tauri/tauri.conf.json` under `plugins.updater.pubkey` (handled in ADR-0024).

The updater key is independent of OS code signing. It's a free keypair generated by Tauri's CLI. OTA updates work even though the binaries are OS-unsigned because the updater plugin verifies signatures against `pubkey` before applying the payload; what Gatekeeper / SmartScreen warn about is a separate concern that affects the install / launch UX, not the integrity check the updater performs.

### Three review gates

1. **Prepare-release PR diff.** Maintainer reviews the bumped versions in `package.json` / `Cargo.toml` / `tauri.conf.json`, the composed changelog notes, and any auxiliary version surfaces. Catches typos in the version, wrong release notes, missing entries.
2. **Draft GitHub Release.** Maintainer downloads at least one artefact per platform and confirms it launches (after the documented click-through on macOS / Windows). Edits the release body if the auto-composed notes need polish. Catches broken updater signing, missing artefacts, wrong tag.
3. **Publish click.** The release stays draft (not public, not consumed by the updater manifest) until the maintainer clicks Publish in the GitHub UI. ADR-0024's manifest regeneration fires on `release: published`, so the manifest only ever points at reviewed artefacts.

### Required repository secrets

The maintainer must provision both of the following before the first `release.yml` run; missing secrets cause the build to fail with a clear error:

| Secret | Purpose |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | passphrase for the updater key |

`GITHUB_TOKEN` is provided automatically by Actions and is sufficient for the prepare-release PR and the draft Release upload; no PAT needed.

### Manifest URL pattern (for ADR-0024 to consume)

The release pipeline emits artefacts named so that ADR-0024's manifest regeneration can resolve them deterministically. The updater manifest URL pattern the app will be configured with is:

```
https://<gh-pages-host>/updater/{{target}}/{{current_version}}
```

`{{target}}` is the Tauri runtime placeholder for the platform triple (e.g. `darwin-aarch64`, `windows-x86_64`); `{{current_version}}` is the installed version. The manifest is served from GitHub Pages, regenerated on the `release: published` event. Hosting choice, manifest schema, and regeneration workflow are decided in ADR-0024; this ADR locks the URL pattern so the pipeline knows where to put the bundles and the app knows where to look.

## Consequences

### Positive

- Three gates between any commit landing and a public binary going out. No single mistake (wrong version, broken build) reaches end users without a chance to catch it.
- No recurring cert costs. No Apple Developer Program signup, no commercial Windows cert, no SignPath application. The project ships for free.
- Two secrets to provision instead of eight. Rotating either is the only operational signing task.
- The updater key works uniformly across platforms; ADR-0024 doesn't have to special-case per-OS verification.
- The split workflow scales: prepare-release runs in seconds (text edits), release.yml runs in tens of minutes (builds) and only on tag push.

### Negative

- macOS users see a Gatekeeper warning on first launch and on the first launch after every OTA update. They right-click > Open (or run `xattr -d com.apple.quarantine`). README docs (#306) walk through it.
- Windows users see a SmartScreen warning on the same cadence. Same documented click-through.
- Tag-cutting takes two manual steps from the maintainer (run prepare-release, then merge the PR) plus a third (publish the draft). Acceptable; the maintainer wants the gates.
- User trust is lower than a signed app would be. Some users will not install software with a Gatekeeper warning at all; that's the trade-off accepted by going non-profit and unsigned.

### Neutral / follow-ups

- ADR-0024 will document the updater manifest hosting, schema, regeneration workflow, and the rollback story. It consumes the URL pattern above and the public key emitted by the keypair here.
- README install docs (#306) document the macOS Gatekeeper and Windows SmartScreen click-throughs as the canonical install steps.
- Revisit signing if the project shifts away from non-profit, if SignPath OSS approves the project, or if user feedback shows the click-through friction is dropping too many would-be users. The change is additive: OS code-signing certs can be added later without breaking the updater key, since the keypair remains the source of truth for OTA verification.
- ARM64 Windows / Linux and `.rpm` get a fresh issue if and when demand justifies the matrix expansion.

## References

- [Tauri 2 - Distribution](https://v2.tauri.app/distribute/) - upstream signing and bundle guidance.
- [Tauri 2 - Updater plugin](https://v2.tauri.app/plugin/updater/) - the consumer for the keypair generated here.
- [`tauri signer`](https://v2.tauri.app/reference/cli/#signer) - keypair generation CLI.
- [SignPath OSS programme](https://signpath.org/products/foundation) - a future Windows-signing path if the project revisits OS-level signing.
- ADR-0024 (forthcoming) - updater manifest hosting and consumption.
- Issue #303 - `prepare-release.yml` implementation.
- Issue #304 - `release.yml` implementation.
- Issue #306 - install + first-run README docs (Gatekeeper / SmartScreen).
