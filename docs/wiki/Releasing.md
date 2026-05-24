# Releasing

How a PRism release gets cut. v1.0.0 is the first stable tag; the same playbook applies to every subsequent release.

The pipeline shape, signing choices, and review gates are recorded in [ADR-0023](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0023-release-pipeline.md) and [ADR-0024](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0024-auto-update-mechanism.md). This page is the operational checklist.

## One-time setup (before the first release)

These steps need to happen exactly once per repository.

### 1. Generate the Tauri updater keypair

The updater key signs every installer so the auto-update plugin can verify downloads. It is independent of OS code signing (which the project does not use - see ADR-0023).

```sh
tauri signer generate -w ~/.tauri/prism-updater.key
```

Capture both halves:

- **Private key** -> GitHub repo secret `TAURI_SIGNING_PRIVATE_KEY`.
- **Password** -> GitHub repo secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- **Public key** -> commit into `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`, replacing the `"REPLACE_WITH_GENERATED_PUBLIC_KEY"` placeholder. Open a small PR for this; CC title `chore(tauri): updater pubkey`.

Keep the local copy of the private key safe. Losing it means cutting a new keypair, which invalidates auto-update for anyone already on the old key (they will need to reinstall from a fresh download).

### 2. Configure GitHub Pages

The updater manifest (`latest.json`) ships from the `gh-pages` branch via `.github/workflows/update-manifest.yml`.

- Repo Settings -> Pages -> **Source: `gh-pages` branch, `/` root**.

The first `update-manifest.yml` run will fail with "branch does not exist" until that setting is in place; subsequent runs publish cleanly.

### 3. Confirm the required secrets

Repo Settings -> Secrets and variables -> Actions. Confirm:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

No other secrets are required - the release pipeline is unsigned at the OS level on macOS and Windows (per ADR-0023).

## Cutting a release

### 1. Platform-QA on the current `main`

Walk [Platform-QA](Platform-QA) on macOS, Windows, and Linux before tagging. The launch surfaces (fresh install, reauth, notifications, sync, window chrome, theme) need to read clean on every platform.

### 2. Trigger `prepare-release.yml`

GitHub Actions UI -> `Prepare release` workflow -> **Run workflow**:

- `bump`: `patch` | `minor` | `major`. For the first stable tag, use `major` (`0.x.y` -> `1.0.0`).
- `version`: leave blank unless you need to override the computed bump.

The workflow opens a PR titled `chore(release): vX.Y.Z` with:

- `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` bumped consistently.
- `CHANGELOG.md`'s `[Unreleased]` block promoted to `[vX.Y.Z] - YYYY-MM-DD`, with a fresh empty `[Unreleased]` above.
- The PR body holds the composed notes grouped by Conventional Commit type (Added / Fixed / Changed / Documentation / Misc).

### 3. Review the release PR

This is review gate 1. Check:

- Correct version in all three files.
- Changelog body reads as you want users to see it. Edit the file directly in the PR if needed; the workflow only seeded the first draft.
- No surprises in the diff.

Squash-merge to `main`. The post-merge `tag-on-release-merge.yml` workflow detects the release-PR shape and pushes the `vX.Y.Z` tag automatically.

### 4. Wait for `release.yml` to produce the draft release

The tag push triggers `release.yml`, which builds a four-platform matrix:

- `macos-13` (Intel) and `macos-14` (Apple Silicon) -> `.dmg` + `.app`.
- `windows-latest` -> `.msi`.
- `ubuntu-22.04` -> `.AppImage` + `.deb`.

All artefacts are signed by the Tauri updater key (the same one from setup step 1) and uploaded to a **draft** GitHub Release at the new tag. Wait for the workflow to complete - the Rust matrix legs are the slow ones, typically several minutes per OS.

### 5. Review the draft release

This is review gate 2. Open the draft in the GitHub UI and:

- Confirm each platform's artefacts are present and named correctly.
- Download at least one artefact per platform; smoke-test that it launches. On macOS, this means right-click -> Open the first time; on Windows, "More info -> Run anyway" in SmartScreen.
- Edit the release notes body if the composed text needs polish.

### 6. Click Publish

This is review gate 3. Publishing the draft fires the `release: published` event, which triggers `update-manifest.yml`. That workflow:

- Downloads the `.sig` files from the release.
- Composes `latest.json` with `version`, `pub_date`, and `platforms.{darwin-aarch64, darwin-x86_64, windows-x86_64, linux-x86_64}` entries pointing at the release asset URLs and bundling the signatures.
- Pushes the manifest to the `gh-pages` branch root.

Within a minute or so of pushing, `https://cerinoligutom.github.io/PRism/latest.json` reflects the new release. Users who have opted into auto-update (Settings -> Updates) pick it up on their next scheduled check.

## Smoke-testing the updater end-to-end

Once a real release has shipped, verify the OTA path works:

1. Install the published binary on a fresh machine (or wipe the local DB).
2. Open the app, complete onboarding, then go to Settings -> Updates and toggle "Automatically check for updates" ON.
3. Cut a small follow-up release (e.g. `vX.Y.(Z+1)` with a one-line CHANGELOG entry).
4. Click "Check now" in Settings -> Updates on the running app. The "update available" banner should surface within a few seconds.
5. Click "Install now" or "Install on next quit". The app relaunches into the new version. Confirm the StatusBar version pill reflects the new SemVer + commit SHA.

If the check fails, the panel surfaces "Last check failed: <reason>" silently. The most likely cause is a signature mismatch (rotated keypair without rebuilding) or a manifest 404 (Pages config not pointing at `gh-pages` yet).

## When things go wrong

### A platform leg failed in `release.yml`

The matrix uses `fail-fast: false`, so the other platforms still upload. Investigate the failed job, push a fix to `main`, then either:

- **Re-run the failed leg only**: Actions UI -> the failed workflow run -> "Re-run failed jobs". The fix needs to be a no-op for the already-uploaded artefacts (e.g. the failed leg's local toolchain issue, not a code bug).
- **Delete the draft + re-tag**: `gh release delete vX.Y.Z`, then `git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`, then trigger `prepare-release.yml` again. Cleaner when the failure was a real code bug that needed a follow-up commit.

### Manifest workflow failed after publish

`update-manifest.yml` runs after Publish. If it fails (most commonly a missing `.sig` file because `bundle.createUpdaterArtifacts: true` got accidentally removed from `tauri.conf.json`), the published release is fine but the updater manifest is stale. Fix the underlying issue, then re-run the workflow: Actions UI -> `Update manifest` -> the failed run -> "Re-run all jobs".

### Need to pull a release

`gh release delete vX.Y.Z --cleanup-tag` removes both the release and the tag. Users already on that version stay on it (the updater won't downgrade them); the manifest at `gh-pages` continues serving whatever was last published.

## Outstanding follow-ups

Tracked separately, not blocking v1.0:

- **gitleaks CI flake**: `gitleaks-action@v2` intermittently fails its license validation step on PRs (HTTP 401 on the user-lookup call). Either apply for the free OSS licence at <https://gitleaks.io>, pin the action to a pre-license version, or replace the action wrapper with the `gitleaks` binary directly in a workflow step.
- **SignPath OSS application**: would remove the Windows SmartScreen warning each release. Apply at <https://signpath.org/products/foundation>; once approved, add a sign step to `release.yml`'s Windows leg and update ADR-0023.
- **macOS code signing**: if the project's funding situation shifts, an Apple Developer ID + notarisation removes the Gatekeeper click-through. Costs $99/yr and is fully additive to the current pipeline.

## References

- [ADR-0022](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0022-versioning-and-build-metadata.md) - SemVer scheme + version sync + build metadata.
- [ADR-0023](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0023-release-pipeline.md) - release pipeline + signing strategy.
- [ADR-0024](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0024-auto-update-mechanism.md) - auto-update mechanism + opt-in default.
- [ADR-0025](https://github.com/cerinoligutom/PRism/blob/main/docs/adr/0025-in-app-changelog.md) - in-app "What's new" dialog.
- [Platform-QA](Platform-QA) - per-platform pre-release checklist.
