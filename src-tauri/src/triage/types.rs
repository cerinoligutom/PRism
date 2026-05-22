//! DTO types for the triage query surface.
//!
//! Mirrors the TypeScript shapes documented in
//! `docs/contracts/triage-ux.md`. Field names are `snake_case` because Rust
//! serde emits them verbatim from struct fields; the frontend mirror reads
//! the same wire shape.

use serde::{Deserialize, Serialize};

/// Filter chip identifier. The wire shape is the kebab-case `ChipKey` from
/// `docs/contracts/triage-ux.md` ("Frontend component interfaces"); the
/// dashboard command takes a `Vec<ChipKey>` as the active chip set and the
/// counts command projects one count per variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChipKey {
    NeedsAttention,
    UnresolvedThreads,
    CiFailing,
    Stale,
    Drafts,
}

/// Counts for the dashboard's filter-chip row. Each chip's count is
/// independent of the active chip filter set so the user always sees what
/// would match if they toggled a single chip alone. The view-scope still
/// applies (Authored / Assigned / Watching / Tracked) because the chips never
/// cross view boundaries.
///
/// Definitions:
///
/// - `needs_attention`: precomputed `pull_request_viewer_relations.needs_attention`
///   column - see ADR 0015 ("Composite formula") for the four input conditions.
/// - `unresolved_threads`: PRs with
///   `threads_unresolved_involved + threads_unresolved_uninvolved > 0`.
/// - `ci_failing`: PRs with `ci_state IN ('FAILURE', 'ERROR')`.
/// - `stale`: PRs with `(now - updated_at) > 7 days`. The 7-day window is
///   pinned in the contract; the sync cycle does not pre-aggregate this
///   because it depends on wall-clock at read time.
/// - `drafts`: PRs with `is_draft = 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterChipCounts {
    pub needs_attention: i64,
    pub unresolved_threads: i64,
    pub ci_failing: i64,
    pub stale: i64,
    pub drafts: i64,
}

/// Per-view counts of PRs flagged `needs_attention = 1` for the active
/// account. The sidebar nav uses these to boost the count chip with the
/// existing `.has-attention` class when any matching PR is outstanding in
/// that view. Mirrors the four `DashboardView` variants.
///
/// The Tracked view's count is account-scoped through the same join the
/// dashboard query uses: only PRs the active account has a relation row for
/// contribute (because `needs_attention` is per-account). The Tracked view's
/// repo-tracking flag still gates which PRs are eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SidebarAttentionCounts {
    pub authored: i64,
    pub assigned: i64,
    pub watching: i64,
    pub tracked: i64,
}
