//! DTO types for the triage query surface.
//!
//! Mirrors the TypeScript shapes documented in
//! `docs/contracts/triage-ux.md`. Field names are `snake_case` because Rust
//! serde emits them verbatim from struct fields; the frontend mirror reads
//! the same wire shape.

use serde::{Deserialize, Serialize};

/// Counts for the dashboard's filter-chip row. Each chip's count is
/// independent of the active chip filter set so the user always sees what
/// would match if they toggled a single chip alone. The view-scope still
/// applies (Authored / Assigned / Watching / Team) because the chips never
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
