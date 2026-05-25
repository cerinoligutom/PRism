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
- "Every 30 minutes", "Every hour", and "Manual" options in Settings -> Sync; Manual (sentinel `sync_interval_seconds = 0`) parks the scheduler so only `Cmd+R` and the Refresh button trigger a cycle, and the status bar shows "On demand" with no "Next in" countdown (#358).
- Dashboard PR row time cell now self-updates every minute (no more frozen "56s ago"), shows a combined tooltip explaining the last-updated timestamp + the status label, and colour-codes the status chip (green for approved, red for failing / changes / conflicts, yellow for stale). Reviewer summary numbers right-align to the column edge instead of hugging a variable-width avatar stack (#362).
- "Open on Unravel" button on each PR row, beside "Open on GitHub". Opens `https://www.unravel.sh/{owner}/{repo}/pull/{number}` externally via `tauri-plugin-opener` (#368).
- Conversation stats sidebar (oldest unresolved, avg response, resolution rate, comments, participants, reviews submitted, last activity) is now visible on every PR detail tab in both drawer and detail-route modes; a new "Comments" tab renders the PR-level issue comments via the existing markdown pipeline (#408).

### Changed

- Backend logs now flow through the `tracing` crate; default level is `warn` and `RUST_LOG` overrides apply (#334).
- Multi-account dashboard rows show the union of involved threads across every tracked account; a thread visible to two accounts counts once and the threads-bar matches the unified view's account scope (#338).
- Status timeline tab now renders the wider GitHub event set per ADR-0027: label add/remove, assignee add/remove, milestone add/remove, force-pushes, base-branch changes, and lock/unlock (#342).
- Renamed the "Assigned to me" sidebar entry and view title to "Review requested" so the label matches the underlying `is_review_requested` predicate (and doesn't get confused with GitHub's `assignees` array); route slug is now `/dashboard/review-requested`. Every view also gained a one-line subtitle explaining what's inside (#366).
- Threads, Reviews, and Comments tabs in the PR detail view now share a consistent bordered + filled card chrome with an accent-colour border on hover, and the comment / review body and author lines step up one token to the 14px design-system body size for more comfortable reading. The conversation stats sidebar also gets its own scroll body so it stays visible while the active tab scrolls under it. The state-badge tooltip on each thread now renders the same INVOLVED / OUTDATED chip pills used inline, and the Threads tab header gains an info button explaining the badge colours and chip meanings (#413).

### Deprecated

### Removed

### Fixed

- Conversation drawer / route refreshes in place when a sync cycle completes; cached entries for non-visible PRs drop so the next open re-hydrates (#337).
- Dashboard "Refresh now" / "Try again" buttons trigger a sync cycle instead of re-running the local DB query, matching the `Cmd/Ctrl+R` keyboard binding (#356).
- Dashboard PR row polish: time column drops the hour remainder past a week (so "120d" no longer wraps), reviewer-stack tooltip hit area matches the visible avatars, and the `changes / approved / total` summary numbers share a single vertical baseline (#360).
- Group header "active X ago" reflects the latest activity in the group (matches the freshest row and updates when the group filter flips between repo / org / none) and shows a single combined tooltip instead of two stacked chips (#364).
- Clicking a row in the Notifications inbox now opens the PR in drawer mode as well as route mode; the handler navigates to the matching dashboard view before expanding the drawer so the host is mounted (#400).
- Clicking a desktop OS notification now honours the active detail surface (drawer mode opens the drawer; route mode opens the detail route, unchanged). The desktop, deep-link, and inbox click paths now share one routing helper so future surface changes only need one edit (#410).
- Sync cycle now surfaces a warn-level activity row + tracing log when the PR-detail GraphQL response resolves `repository.pullRequest` to null, carrying the PR coordinates and a 256-byte body excerpt so the silent miss (which leaves detail-derived columns and the conversation tables empty) is diagnosable without enabling `RUST_LOG` (#402).
- Removing an account now also purges the `etags` rows the sync worker stamped under that account's key prefix; the accounts DELETE alone left every cached REST + GraphQL entry on disk because `etags` has no foreign key on `accounts` (#405).
- Sync cycle no longer stamps the pre-flight, post-flight body-hash, or repair cache markers for a PR when the detail GraphQL response resolves `repository.pullRequest` to null; previously all three were written regardless, locking the empty state in so future cycles skipped the fetch and the conversation tables stayed empty forever. The next cycle now retries on the normal path until discovery prunes the relation or GitHub returns the full payload (#403).

### Security
