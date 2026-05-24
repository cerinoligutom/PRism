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

### Changed

- Backend logs now flow through the `tracing` crate; default level is `warn` and `RUST_LOG` overrides apply (#334).

### Deprecated

### Removed

### Fixed

### Security
