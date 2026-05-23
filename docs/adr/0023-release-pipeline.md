# 0023 - Release pipeline: two-workflow split, draft GitHub Release, signing per platform

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#297](https://github.com/cerinoligutom/PRism/issues/297)
- **Deciders:** @cerinoligutom

## Context

PRism is a Tauri 2 desktop app shipping to macOS (Intel + Apple Silicon), Windows x64, and Linux x64. v1.0 is days away from tag-cutting. The release path has been ad-hoc up to now (local `tauri build`, manual artefact uploads), which doesn't scale once the updater (ADR-0024) starts pulling a manifest off published releases on every app start.

Three concerns have to be settled before a v1.0 tag goes near the repo:

- **Trigger and review shape.** Anything that publishes a binary the public can download has to pass through human review. A direct push-to-tag-then-publish pipeline removes the review surface; tag-on-main with a draft release adds two.
- **Code signing per platform.** macOS Gatekeeper hard-rejects unsigned apps without an explicit user override. Windows SmartScreen warns on unsigned binaries but is click-through. Linux has no OS-level signing requirement, but the Tauri updater ships its own signature scheme that all platforms participate in.
- **Updater integration.** The updater plugin needs a signing keypair and a manifest URL. ADR-0024 owns the plugin and the manifest hosting; ADR-0023 owns the pipeline that produces signed artefacts and the URL pattern the manifest will follow.

## Decision drivers

- v1.0 is a launch, not a hotfix. Maintainer wants explicit review gates at three points: bumped versions, built binaries, and the final publish click.
- macOS unsigned + un-notarised is a non-starter (Gatekeeper blocks first launch behind a Settings > Privacy override that real users won't find). Apple Developer ID + notarisation is already paid for and provisioned.
- Windows code-signing certificates for an OSS hobby project are either expensive (commercial EV at hundreds per year) or come with friction (SignPath OSS programme requires application + approval). SmartScreen warns but doesn't block. Acceptable for v1.0 with documented click-through, with SignPath as a follow-up.
- Linux AppImage has no OS-level signing, but Tauri's own updater signature still applies (the updater verifies all downloaded artefacts before swapping them in). One keypair covers the signing-by-updater requirement across every platform.
- Two failure modes to guard against: a bad version bump going public (catch at PR review), and a broken binary going public (catch in the draft release).

## Considered options

1. **Two-workflow split with draft Release (chosen).** `prepare-release.yml` opens a PR with version bumps and composed changelog; merging that PR tags `vX.Y.Z`; `release.yml` builds the matrix and attaches binaries to a *draft* GitHub Release; the maintainer publishes manually.
2. **Single workflow on tag push.** One workflow that, on `vX.Y.Z` tag, builds and immediately publishes a non-draft Release. Loses the artefact-review gate; if the build is broken or signing failed silently the public sees it.
3. **Sign Windows now via a paid cert or SignPath OSS.** Pay for a commercial EV cert or apply to SignPath. Removes SmartScreen warnings.

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

Each matrix leg signs its artefacts and uploads them to a single **draft** GitHub Release named `vX.Y.Z`. The maintainer reviews the draft (artefacts present, names correct, edits release notes if needed) and clicks **Publish** when satisfied. Publishing emits the `release: published` event that ADR-0024's pipeline will react to.

### OS coverage

- macOS Intel + Apple Silicon: `.dmg` (drag-to-install) and `.app` (raw bundle for the updater payload). Both notarised.
- Windows x64: `.msi` only. No `.exe` from the NSIS bundler in v1.0.
- Linux x64: `.AppImage` (portable) and `.deb` (Debian/Ubuntu). No `.rpm` in v1.0.

ARM64 Windows and Linux are out of scope for v1.0.

### Code signing

- **macOS:** Apple Developer ID (Application) certificate + notarytool notarisation + stapling. Cert lives in the `APPLE_CERTIFICATE` secret as a base64-encoded `.p12`; the password in `APPLE_CERTIFICATE_PASSWORD`; the signing identity name in `APPLE_SIGNING_IDENTITY`; the notarisation Apple ID, app-specific password, and team ID in `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.
- **Windows:** unsigned for v1.0. README install docs (#306) document the SmartScreen "More info > Run anyway" click-through. SignPath OSS application is tracked as a separate follow-up; once approved, the `.msi` build step adds a sign step ahead of the upload.
- **Linux AppImage:** no OS-level code-signing cert. The Tauri updater key (below) signs every AppImage, which is what the updater plugin verifies on download.

### Tauri updater keypair

Generated once via `tauri signer generate` and stored as `TAURI_SIGNING_PRIVATE_KEY` (the encoded private key) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the passphrase) in repository secrets. This pair signs **every installer on every platform** so the updater can verify downloads regardless of OS. The public counterpart is committed to `src-tauri/tauri.conf.json` under `plugins.updater.pubkey` (handled in ADR-0024).

The updater key is separate from the macOS code-signing identity. Two distinct signing systems running side by side on the same artefact: Apple verifies the macOS bundle, the Tauri updater verifies any platform's bundle.

### Three review gates

1. **Prepare-release PR diff.** Maintainer reviews the bumped versions in `package.json` / `Cargo.toml` / `tauri.conf.json`, the composed changelog notes, and any auxiliary version surfaces. Catches typos in the version, wrong release notes, missing entries.
2. **Draft GitHub Release.** Maintainer downloads at least one artefact per platform and confirms it launches. Edits the release body if the auto-composed notes need polish. Catches broken signing, missing artefacts, wrong tag.
3. **Publish click.** The release stays draft (not public, not consumed by the updater manifest) until the maintainer clicks Publish in the GitHub UI. ADR-0024's manifest regeneration fires on `release: published`, so the manifest only ever points at reviewed artefacts.

### Required repository secrets

The maintainer must provision all of the following before the first `release.yml` run; missing secrets cause the matrix legs that need them to fail with a clear error:

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE` | base64-encoded Developer ID `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password for the `.p12` |
| `APPLE_SIGNING_IDENTITY` | identity name (e.g. `Developer ID Application: Name (TEAMID)`) |
| `APPLE_ID` | Apple ID email for notarisation |
| `APPLE_PASSWORD` | app-specific password for notarytool |
| `APPLE_TEAM_ID` | Apple Developer team ID |
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

- Three gates between any commit landing and a public binary going out. No single mistake (wrong version, broken build, half-signed artefact) reaches end users without a chance to catch it.
- macOS first-launch UX is clean: signed, notarised, stapled, no Gatekeeper override required.
- The updater key works uniformly across platforms; ADR-0024 doesn't have to special-case per-OS verification.
- The split workflow scales: prepare-release runs in seconds (text edits), release.yml runs in tens of minutes (builds) and only on tag push.

### Negative

- Windows users see a SmartScreen warning on first launch through v1.0. Mitigated by README copy and the SignPath follow-up; not mitigated for the release itself.
- Tag-cutting takes two manual steps from the maintainer (run prepare-release, then merge the PR) plus a third (publish the draft). Acceptable; the maintainer wants the gates.
- Eight secrets is a lot of provisioning. Audit-friendly because they're listed above, but rotating any of them is a separate operational task.
- Apple's notarisation can take minutes to tens of minutes per artefact. The release.yml matrix wait time is dominated by notarisation, not by `cargo build`.

### Neutral / follow-ups

- ADR-0024 will document the updater manifest hosting, schema, regeneration workflow, and the rollback story. It consumes the URL pattern above and the public key emitted by the keypair here.
- README install docs (#306) document the macOS Gatekeeper first-launch flow (none expected with notarisation) and the Windows SmartScreen click-through.
- Once SignPath OSS approves, add a Windows sign step in `release.yml` and supersede the "unsigned Windows" wording in this ADR with a follow-up.
- ARM64 Windows / Linux and `.rpm` get a fresh issue if and when demand justifies the matrix expansion.

## References

- [Tauri 2 - Distribution](https://v2.tauri.app/distribute/) - upstream signing and bundle guidance.
- [Tauri 2 - Updater plugin](https://v2.tauri.app/plugin/updater/) - the consumer for the keypair generated here.
- [`tauri signer`](https://v2.tauri.app/reference/cli/#signer) - keypair generation CLI.
- [SignPath OSS programme](https://signpath.org/products/foundation) - the Windows-signing follow-up path.
- [Apple notarytool](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow) - notarisation reference.
- ADR-0024 (forthcoming) - updater manifest hosting and consumption.
- Issue #303 - `prepare-release.yml` implementation.
- Issue #304 - `release.yml` implementation.
- Issue #306 - install + first-run README docs (Gatekeeper / SmartScreen).
