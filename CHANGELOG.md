# Changelog

All notable changes to PRism are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

PRs that introduce user-facing changes append entries to the `[Unreleased]` section under the matching subheading (Added / Changed / Deprecated / Removed / Fixed / Security). Release tooling promotes `[Unreleased]` to a versioned block at release time; see `scripts/stamp-changelog.ts`.

## [Unreleased]

### Added

- Auto-update toggle in Settings, opt-in by default (#308).
- In-app "What's new" dialog (#305).
- `E` keyboard shortcut to archive (or unarchive on the Archive view) the focused PR row; arrow keys move the focus highlight through the list (#332).
- "Mark all read" action in the dashboard header clears unread / mention state on every PR matching the active view + chip filter (#336).
- Bulk multi-select on dashboard rows with an inline checkbox; the toolbar above the list archives every selected PR in one batched write per account, with Shift+click for range extension (#331).
- Configurable auto-archive window in Settings -> Sync; defaults to 30 days, 0 disables auto-archive, capped at 365 (#333).
- Cross-platform unread badge parity: Windows taskbar overlay icon and Linux Unity launcher D-Bus count signal, fed by the same `count_global_unread` query the macOS dock badge already uses (#330).
- `prism://` custom URL scheme for deep-linking into a specific PR; URL shape `prism://pr/<owner>/<repo>/<number>` with an optional `?host=` query for non-github.com PRs, falling back to opening on GitHub when PRism isn't tracking the PR (#339).
- Hover tooltip on the `PRismAvatarStack` overflow pill listing the hidden logins, with one-line / per-line / "...and N more" formatting based on count (#341).

### Changed

- Backend logs now flow through the `tracing` crate; default level is `warn` and `RUST_LOG` overrides apply (#334).
- Multi-account dashboard rows show the union of involved threads across every tracked account; a thread visible to two accounts counts once and the threads-bar matches the unified view's account scope (#338).

### Deprecated

### Removed

### Fixed

- Conversation drawer / route refreshes in place when a sync cycle completes; cached entries for non-visible PRs drop so the next open re-hydrates (#337).

### Security
