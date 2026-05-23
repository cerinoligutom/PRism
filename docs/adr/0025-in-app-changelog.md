# 0025 - In-app changelog: bundled `CHANGELOG.md`, last-seen version gate, single concatenated dialog

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#302](https://github.com/cerinoligutom/PRism/issues/302)
- **Deciders:** @cerinoligutom

## Context

As PRism evolves past v1 into the auto-update cadence (M10), users need an in-app surface that tells them what changed since the previous launch. The alternative is "check GitHub Releases when you remember to" - which the bug-report stream already shows users don't do; "what changed in this version?" is a recurring question.

A read-only desktop app benefits from passive discoverability: a dismissable "What's new" dialog that surfaces on launch after the version has moved, then gets out of the way. The pieces it depends on already exist:

- `CHANGELOG.md` at the repo root will land in issue #296 (PR #310 in flight). Keep-a-Changelog format, version-stamped via the existing release pipeline.
- The comment-markdown pipeline (ADR 0014) already renders sanitised HTML via GitHub `bodyHTML` + Shiki client highlighting; the same DOMPurify pass and Shiki theme apply to local markdown.
- `PRismDialog` (Reka `DialogRoot` + the design-system primitives) is the existing modal affordance.
- The settings persistence boundary (ADR 0020) gives a clear home for the last-seen-version cursor.

The dialog implementation lives in issue #305; this ADR records the shape so that issue can ship without relitigating the design.

## Decision drivers

- Offline-friendly. A launch-time network call to GitHub Releases re-introduces a dependency on github.com being reachable, which the rest of the app doesn't have at launch (the sync worker tolerates offline starts and the dashboard renders from cache). A GitHub outage shouldn't break the launch UX.
- Local-first ethos. PRism caches PR state locally and only writes to GitHub via the user's browser. The release notes the user is reading should match the binary they're running; that's only true if the changelog ships inside the binary.
- Render reuse. Markdown rendering is already a solved problem in the codebase. Anything that doesn't reuse it pays a sanitisation cost twice.
- Respect for the user. The dialog is dismissable, fires once per version transition, and never appears on first install. No nag, no opt-in flow, no toast follow-up.
- Single source of truth. The dialog body and the changelog file must not be able to drift. Bundling the file at build time makes drift impossible by construction.

## Considered options

1. **Bundled `CHANGELOG.md` + `last_seen_version` gate + single concatenated dialog covering all skipped versions.** Markdown lives in the binary; on launch, compare `app_metadata.version` against `settings.last_seen_version`; if newer, render the slice of the file covering every version between them in one dialog.
2. **Live GitHub Releases API fetch.** Pull release bodies at launch from `GET /repos/cerinoligutom/PRism/releases`. Fresh content without rebuilding, but requires network and a successful response before the dialog can render.
3. **No in-app surface.** Direct users to GitHub Releases via a menu item.
4. **Per-version individual dialogs.** When the user has skipped multiple versions, show one dialog per version with sequential "Next" clicks.

## Decision

We will go with **Option 1**.

`CHANGELOG.md` is loaded as a Vite-bundled asset (build-time import, no runtime fetch). On app start, the frontend reads the current version from `app_metadata.version` (Tauri-supplied) and compares it against `settings.last_seen_version` (SQLite, per ADR 0020). The comparison is semver-ordered, not string-equal: a user who skipped v0.2.0 and v0.3.0 and now launches v0.4.0 has `last_seen_version = "0.1.0"` and `app_metadata.version = "0.4.0"`.

When `current > last_seen`, open `PRismDialog` titled "What's new in vX.Y.Z" (where X.Y.Z is the current version). The body is a single rendered markdown block that concatenates every changelog section newer than `last_seen_version`, with the per-version headers from `CHANGELOG.md` left intact. One scrollable read-through, one dismiss.

The dialog footer carries a "View full changelog on GitHub" link that opens the release page via `tauri-plugin-opener` (the same plugin used for the existing GitHub-issue and PR-permalink openers). Dismissing the dialog (close button, Esc, or backdrop click) persists `last_seen_version = current version` to SQLite.

**First-install rule.** When `last_seen_version` is unset (fresh install, no prior cursor), the dialog does not open. Instead the frontend records `last_seen_version = current version` on first launch and exits the codepath. A user installing v0.4.0 for the first time should not see "What's new in v0.4.0 - everything is new" out of the gate; their first encounter with the dialog is when they update to v0.5.0.

**Why bundled over live.** Option 2 trades one form of drift (the file vs. the binary) for two (network availability + Releases API drift). It also makes the dialog feel different from the rest of the app: every other surface degrades gracefully when offline; a changelog modal that says "Failed to fetch release notes" on a flight would be a poor first impression of an otherwise local-first app. The cost of bundling is a CONTRIBUTING checklist line - "update CHANGELOG.md as part of any user-facing PR" - which issue #296 already adds.

**Why single concatenated over per-version.** Option 4 forces the user to click "Next" once per skipped version. For someone who hadn't updated in a month and now spans three releases, that's three dismissals for content they're going to scroll through anyway. A single dialog with per-version sub-headers (`## v0.4.0`, `## v0.3.0`, `## v0.2.0`) is the same content with one dismissal.

## Consequences

### Positive

- The dialog renders with zero network dependency. Works on a plane, on a corporate-VPN block of github.com, on a residential outage.
- Markdown rendering reuses the ADR 0014 pipeline; no second sanitiser, no second Shiki configuration, no new XSS surface to audit.
- First-install users see a clean launch. The dialog earns its first appearance by the user actually updating.
- Multi-version skip is one dialog, one dismiss. Friction scales with launch count, not skipped-version count.
- Bundled file means the release notes the user reads match the binary they're running, by construction.

### Negative

- Every user-facing PR must update `CHANGELOG.md`. Issue #296 adds the CONTRIBUTING reminder; if a maintainer forgets, the next release ships with a changelog gap and the dialog under-reports the changes. Catch is during release-prep review.
- Bundling the file at build time means the dialog content is frozen at release. A typo found post-release isn't fixable until the next version ships. Acceptable given the cadence and the audience size.
- A user who wants to re-read the changelog after dismissing has to open it from the menu (out of scope for this ADR; tracked alongside issue #305 if the need surfaces).

### Neutral / follow-ups

- Dialog implementation lands in issue #305: dialog component, version-comparison helper, changelog-slice extractor, settings wiring, opener-link plumbing.
- The CONTRIBUTING.md edit that pins the "update CHANGELOG.md" expectation lands in issue #296 alongside the file itself.
- If the changelog ever grows large enough that bundling adds noticeable binary weight, revisit: switch to a pruned `CHANGELOG-bundled.md` containing only the last N releases. Not a concern at v1 scale.
- A critical security advisory that needs to reach users who never relaunch would need a separate notification path (e.g. an auto-update prompt with elevated severity). That's a downstream concern, not a reason to redesign this dialog.

## References

- ADR 0014 - Comment markdown rendering (the pipeline this dialog reuses).
- ADR 0020 - Settings persistence boundary (where `last_seen_version` lives).
- Issue #296 - `CHANGELOG.md` file + CONTRIBUTING reminder (in flight, PR #310).
- Issue #305 - Dialog implementation (downstream of this ADR).
- [tauri-plugin-opener](https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/opener) - the external-link opener used by the footer button.
- [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) - the format the bundled file follows.
