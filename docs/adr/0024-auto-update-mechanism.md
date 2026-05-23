# 0024 - Auto-update mechanism: tauri-plugin-updater, opt-in default, GH Pages manifest, silent failure

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#301](https://github.com/cerinoligutom/PRism/issues/301)
- **Deciders:** @cerinoligutom

## Context

PRism is a desktop binary the user installs from a GitHub Release. Once installed, there's no signal that a newer version exists short of revisiting the Releases page. For v1.0 that's tolerable; for the v1.x cadence we want the update path to be one click after the user has opted in.

ADR-0023 commits to signed, notarised bundles produced by a tag-driven release pipeline and reserves the manifest URL pattern `https://<gh-pages-host>/updater/{{target}}/{{current_version}}`. This ADR picks the mechanism that consumes that URL, the default behaviour, and the failure UX. Plugin integration is the subject of issue #308; the manifest-generation workflow is the subject of issue #307.

## Decision drivers

- **Privacy / user control.** The app is read-only and observes GitHub on the user's behalf; surprise self-updates contradict that stance. The user has to opt in.
- **Operational cost.** Hosting a manifest endpoint per platform/arch should not require new infrastructure. GH Pages is already configured for the wiki and costs nothing extra.
- **Offline-friendly.** When the toggle is off, the app never reaches out for an update check. No background network on launch, no telemetry side-channel.
- **Small attack surface.** Updates must be verifiable end-to-end. The Tauri updater signature (ADR-0023's `TAURI_SIGNING_PRIVATE_KEY`) and the OS-level installer signature (Apple Developer ID on macOS) both apply; the manifest itself is served over HTTPS from a GH Pages origin we control.
- **Match the existing settings shape.** Sync interval already persists via the settings boundary (ADR-0020); the updater toggle and interval follow the same pattern so there's one rule for "settings the worker reads".

## Considered options

1. **`tauri-plugin-updater` + opt-in default + GH Pages manifest + silent failure (chosen).** First-party Tauri 2 plugin, signed end-to-end, manifest served from a static GH Pages site regenerated on `release: published`.
2. **`tauri-plugin-updater` + opt-OUT default.** Same mechanism, default ON. Rejected: an observer tool shouldn't restart itself without consent. Long debug sessions or workflows that depend on a specific installed version (e.g. someone reproducing a bug against `v1.0.3`) shouldn't have the binary swap underneath them.
3. **Manual download from GitHub Releases only.** Workable for v1.0 and what we have today, but doesn't scale to a v1.x cadence where minor releases will land more often than users will check Releases unaided. Rejected as the long-term answer; it remains the fallback when the toggle is off.

## Decision

We will go with **Option 1**.

### Plugin

[`tauri-plugin-updater`](https://v2.tauri.app/plugin/updater/) (Tauri 2 first-party). The plugin's public key is committed to `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`. Private signing keys (`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) live in repository secrets and are consumed by `release.yml` per ADR-0023.

### Manifest hosting and URL pattern

GitHub Pages serves a static manifest tree under `/updater/{{target}}/{{current_version}}`. The full URL pattern (locked by ADR-0023) is:

```
https://<gh-pages-host>/updater/{{target}}/{{current_version}}
```

`{{target}}` is the Tauri platform triple (e.g. `darwin-aarch64`, `windows-x86_64`, `linux-x86_64`); `{{current_version}}` is the version the running binary reports. The Pages site regenerates on the `release: published` event - the third review gate from ADR-0023 - so the manifest only ever points at artefacts the maintainer has reviewed and published.

The manifest schema, regeneration workflow, and rollback path are scoped to issue #307. This ADR commits only to (a) the URL pattern above and (b) "Pages, not a separate host" as the hosting choice.

### Default behaviour and check cadence

- **Default OFF.** The first-run state of "Auto-update" is disabled. The Settings -> Updates panel shows an explanation of what gets sent to where when enabled, plus a "Check for updates now" button that always works regardless of toggle state.
- **Cadence when enabled: 6 hours.** A background interval check fires every 6h while the app is running. No exponential drift; no jittered cadence. If the app is closed, no check; if the app launches with the toggle on, the first check fires after a 60-second warmup so launch isn't competing for the network with the initial sync cycle.
- **Persistence (per ADR-0020).** Both `auto_update_enabled` (bool) and `auto_update_interval_seconds` (int, default 21600) live in the `app_settings` singleton row in SQLite. The Rust-side updater loop reads them on the same boundary that the sync scheduler already uses; the settings panel writes them through the same Tauri commands that the existing `sync_interval_seconds` setting uses. No new persistence layer.

### UX

- **Check.** Silent. No spinner, no log line surfaced to the user.
- **Update available.** A non-blocking banner appears in the app shell with the new version number, a "What's new" link to the changelog (issue #305 owns this surface), an "Install on next quit" button (default action), and an "Install now" button that triggers immediate download + relaunch.
- **Install on next quit.** The plugin queues the install; when the user next quits the app, the installer runs as part of shutdown. This is the default because it doesn't interrupt the user mid-task.
- **Install now.** Downloads the artefact, verifies signatures (Tauri updater key first; OS-level signatures apply on relaunch), and restarts the app onto the new binary.
- **Manual check** ("Check for updates now" in Settings). Same flow as the background check but surfaces both "no update available" and any error inline in the Updates panel, since the user explicitly asked.

### Failure handling

Failed background checks stay silent. The plugin error is caught, logged to the console with `log::warn!`, and recorded as `last_update_check_failed_at` + `last_update_check_error` in `app_settings`. The Settings -> Updates panel renders a "Last check failed <relative time> ago" line below the toggle when both fields are set; otherwise it shows "Last checked <relative time> ago" or nothing if no check has ever run. No toast, no banner, no modal - this matches the read-only feel of the app and the toast policy established in ADR-0019. The About panel may surface the same line as an additional diagnostic; final placement is left to whoever implements issue #308.

A manual "Check for updates now" failure, by contrast, surfaces the error inline in the panel where the user clicked, because the user opted in to the foreground operation.

### Signing

Installers are signed at release time by the Tauri updater key (`TAURI_SIGNING_PRIVATE_KEY` + `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` per ADR-0023). The plugin verifies the signature against the public key embedded in `tauri.conf.json` before applying any update. macOS bundles additionally carry the Apple Developer ID signature, but the updater key is the cross-platform guarantee; AppImages have no OS-level signature so the updater key is the only verification step on Linux.

## Consequences

### Positive

- Once opted in, users get one-click updates with no manual download. The friction step is "enable in Settings", which is a deliberate moment of consent.
- The settings persistence layer doesn't grow a new shape; the two new fields slot into the existing `app_settings` row alongside `sync_interval_seconds`.
- End-to-end signing: every artefact the user installs is verified twice on macOS (Apple + Tauri updater), once on Linux/Windows (Tauri updater). No unsigned binary path.
- GH Pages costs nothing and lives on the same repo. No external host to provision or rotate credentials for.
- Failed checks don't badger the user. The Settings panel is the single place to learn that something is off.

### Negative

- Linux AppImage updates often require launcher-level support (the AppImage has to be writable and the launcher has to re-exec the new bundle). Tauri's updater handles the common case but some user-launcher setups (read-only mounts, AppImageLauncher integration) may report "update applied" without swapping the binary on disk. Document the limitation in the Settings panel copy and in the install docs (issue #306); the silent-failure rule means an unsuccessful Linux update still won't toast at the user.
- The 6h cadence is a magic number. If a future user wants a tighter or looser interval, the setting is exposed but the only validated value through v1 is 21600. Adding configurable intervals is a follow-up if the trigger fires.
- Opt-in default means most v1 users will never see an update notification unless they go looking. Acceptable trade for the privacy posture; the in-app "What's new" surface (ADR-0025, issue #302) is the counterweight on the discovery side.
- Two settings rows in SQLite for one feature (toggle + interval) is more state than strictly necessary today. Acceptable because (a) the cost is two columns, and (b) the interval field is the natural home if the cadence ever becomes user-configurable.

### Neutral / follow-ups

- Issue #307 implements the GH Pages manifest regeneration workflow and decides the manifest schema.
- Issue #308 implements the `tauri-plugin-updater` integration, the Settings -> Updates panel, and the in-app banner.
- ADR-0025 (issue #302) covers the in-app "What's new" surface that the update banner links to.
- If a future post-mortem traces a user complaint to a silent Linux update failure, revisit "silent on background failure" for the Linux case specifically.
- If we ever ship a security-critical patch where opt-in lag is unacceptable, the existing release pipeline can still publish; the gap is communication (release notes, README) rather than mechanism.

## References

- [Tauri 2 - Updater plugin](https://v2.tauri.app/plugin/updater/) - upstream docs for the plugin chosen here.
- ADR-0017 - Desktop notifications (decision 5: settings stored in `app_settings` for worker visibility - the same pattern this ADR follows).
- ADR-0019 - Error handling convention (no toast on background failure; this ADR applies the rule to update checks).
- ADR-0020 - Settings persistence boundary (SQLite for worker-visible state; the updater loop is a worker reader).
- ADR-0023 - Release pipeline (defines the manifest URL pattern, the signing keypair, and the `release: published` trigger this ADR consumes).
- Issue #307 - Updater manifest hosting on GitHub Pages.
- Issue #308 - `tauri-plugin-updater` integration + Settings "Auto-update" toggle.
- Issue #305 - In-app "What's new" dialog (the link target from the update banner).
